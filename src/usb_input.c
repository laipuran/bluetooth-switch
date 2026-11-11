#include "usb_input.h"
#include "driver/gpio.h"
#include "esp_log.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "sdkconfig.h"
#include "usb/hid_host.h"
#include "usb/usb_host.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

static const char *TAG = "usb_input";
typedef struct
{
    hid_host_device_handle_t handle;
    bool disconnected;
} lifecycle_event_t;
typedef struct
{
    hid_host_device_handle_t handle;
    unsigned source;
    hid_parser_t parser;
    hid_input_t state[HID_MAX_REPORTS];
    unsigned invalid_reports;
} interface_t;
static _Atomic(interface_t *) interfaces[USB_INPUT_SOURCES];
static QueueHandle_t input_queue, connect_queue;
static atomic_bool overflow;
static atomic_uint report_count, active_interfaces;

static void publish(interface_t *ctx, const hid_input_t *input)
{
    usb_input_event_t e = {.source = ctx->source, .input = *input};
    if (xQueueSend(input_queue, &e, 0) != pdTRUE)
        atomic_store(&overflow, true);
}

static void interface_callback(hid_host_device_handle_t handle, hid_host_interface_event_t event, void *arg)
{
    interface_t *ctx = arg;
    if (event == HID_HOST_INTERFACE_EVENT_DISCONNECTED)
    {
        hid_input_t zero = {0};
        publish(ctx, &zero);
        // Lifecycle operations belong to the connection worker. In particular,
        // unplugging during descriptor setup must not free its context here.
        lifecycle_event_t e = {.handle = handle, .disconnected = true};
        if (xQueueSend(connect_queue, &e, 0) != pdTRUE)
            esp_restart();
    }
    else if (event == HID_HOST_INTERFACE_EVENT_TRANSFER_ERROR)
    {
        ESP_LOGW(TAG, "Interface %u transfer error; clearing held input", ctx->source);
        memset(ctx->state, 0, sizeof(ctx->state));
        hid_input_t zero = {0};
        publish(ctx, &zero);
    }
    else if (event == HID_HOST_INTERFACE_EVENT_INPUT_REPORT)
    {
        uint8_t data[1025];
        size_t length = 0;
        if (hid_host_device_get_raw_input_report_data(handle, data, sizeof(data), &length) != ESP_OK)
            return;
        hid_input_t decoded;
        uint8_t index;
        if (!hid_parser_decode(&ctx->parser, data, length, &decoded, &index))
        {
            if (ctx->invalid_reports++ < 3)
                ESP_LOGW(TAG, "Interface %u unknown/truncated report (%u bytes)", ctx->source, (unsigned)length);
            return;
        }
        ctx->state[index] = decoded;
        // Preserve independent report-ID keyboard/button states. Relative motion
        // belongs only to this packet, and must never be replayed from snapshots.
        hid_input_t merged = {.x = decoded.x, .y = decoded.y, .wheel = decoded.wheel, .pan = decoded.pan};
        for (unsigned i = 0; i < ctx->parser.report_count; ++i)
        {
            const hid_input_t *s = &ctx->state[i];
            merged.kinds |= s->kinds;
            merged.buttons |= s->buttons;
            for (unsigned k = 0; k < HID_KEYS_BYTES; ++k)
                merged.keys[k] |= s->keys[k];
            if (!merged.consumer)
                merged.consumer = s->consumer;
        }
        atomic_fetch_add(&report_count, 1);
        publish(ctx, &merged);
    }
}

static void connect_callback(hid_host_device_handle_t handle, hid_host_driver_event_t event, void *arg)
{
    lifecycle_event_t e = {.handle = handle};
    if (event == HID_HOST_DRIVER_EVENT_CONNECTED && xQueueSend(connect_queue, &e, 0) != pdTRUE)
    {
        ESP_LOGE(TAG, "USB connection queue overflow; restarting safely");
        esp_restart();
    }
}

static void connect_task(void *arg)
{
    lifecycle_event_t event;
    for (;;)
    {
        xQueueReceive(connect_queue, &event, portMAX_DELAY);
        hid_host_device_handle_t handle = event.handle;
        if (event.disconnected)
        {
            for (unsigned i = 0; i < USB_INPUT_SOURCES; ++i)
            {
                interface_t *ctx = atomic_load(&interfaces[i]);
                if (!ctx || ctx->handle != handle)
                    continue;
                esp_err_t err = hid_host_device_close(handle);
                if (err != ESP_OK)
                {
                    ESP_LOGW(TAG, "Close: %s", esp_err_to_name(err));
                    break;
                }
                atomic_store(&interfaces[i], NULL);
                atomic_fetch_sub(&active_interfaces, 1);
                ESP_LOGI(TAG, "Interface %u disconnected", i);
                free(ctx);
                break;
            }
            continue;
        }
        unsigned source;
        for (source = 0; source < USB_INPUT_SOURCES; ++source)
            if (!atomic_load(&interfaces[source]))
                break;
        if (source == USB_INPUT_SOURCES)
        {
            ESP_LOGW(TAG, "Too many HID interfaces");
            continue;
        }
        interface_t *ctx = calloc(1, sizeof(*ctx));
        if (!ctx)
        {
            ESP_LOGE(TAG, "No memory for HID parser");
            continue;
        }
        ctx->source = source;
        ctx->handle = handle;
        hid_host_device_config_t cfg = {.callback = interface_callback, .callback_arg = ctx};
        esp_err_t err = hid_host_device_open(handle, &cfg);
        if (err != ESP_OK)
        {
            ESP_LOGW(TAG, "Open: %s", esp_err_to_name(err));
            free(ctx);
            continue;
        }
        hid_host_dev_params_t params;
        err = hid_host_device_get_params(handle, &params);
        size_t size = 0;
        const uint8_t *descriptor = hid_host_get_report_descriptor(handle, &size);
        if (err != ESP_OK || !descriptor || !hid_parser_init(&ctx->parser, descriptor, size))
        {
            ESP_LOGW(TAG, "Unsupported HID descriptor (%u bytes); interface ignored", (unsigned)size);
            if (descriptor)
                ESP_LOG_BUFFER_HEX_LEVEL(TAG, descriptor, size, ESP_LOG_INFO);
            hid_host_device_close(handle);
            free(ctx);
            continue;
        }
        // Report protocol preserves wheel, extended buttons, NKRO and consumer
        // keys; never assume the receiver emits an 8-byte boot report.
        if (params.sub_class == HID_SUBCLASS_BOOT_INTERFACE)
        {
            err = hid_class_request_set_protocol(handle, HID_REPORT_PROTOCOL_REPORT);
            if (err != ESP_OK)
                ESP_LOGW(TAG, "SET_PROTOCOL report: %s", esp_err_to_name(err));
        }
        hid_host_dev_info_t info;
        if (hid_host_get_device_info(handle, &info) == ESP_OK)
        {
            ESP_LOGI(TAG, "USB VID=%04x PID=%04x interface=%u source=%u fields=%u reports=%u", info.VID, info.PID,
                     params.iface_num, source, ctx->parser.field_count, ctx->parser.report_count);
        }
        atomic_store(&interfaces[source], ctx);
        atomic_fetch_add(&active_interfaces, 1);
        err = hid_host_device_start(handle);
        if (err != ESP_OK)
        {
            ESP_LOGW(TAG, "Start: %s", esp_err_to_name(err));
            hid_host_device_close(handle);
            atomic_store(&interfaces[source], NULL);
            atomic_fetch_sub(&active_interfaces, 1);
            free(ctx);
        }
    }
}

static void library_task(void *arg)
{
    for (;;)
    {
        uint32_t flags = 0;
        esp_err_t err = usb_host_lib_handle_events(portMAX_DELAY, &flags);
        if (err != ESP_OK)
            ESP_LOGW(TAG, "USB library: %s", esp_err_to_name(err));
        if (flags & USB_HOST_LIB_EVENT_FLAGS_NO_CLIENTS)
            usb_host_device_free_all();
    }
}

esp_err_t usb_input_init(void)
{
    input_queue = xQueueCreate(128, sizeof(usb_input_event_t));
    connect_queue = xQueueCreate(16, sizeof(lifecycle_event_t));
    if (!input_queue || !connect_queue)
        return ESP_ERR_NO_MEM;
    if (CONFIG_SWITCH_VBUS_GPIO >= 0)
    {
        if (!GPIO_IS_VALID_OUTPUT_GPIO(CONFIG_SWITCH_VBUS_GPIO) || CONFIG_SWITCH_VBUS_GPIO == 19 ||
            CONFIG_SWITCH_VBUS_GPIO == 20 || CONFIG_SWITCH_VBUS_GPIO == CONFIG_SWITCH_BUTTON_GPIO)
            return ESP_ERR_INVALID_ARG;
        gpio_config_t cfg = {.pin_bit_mask = 1ULL << (CONFIG_SWITCH_VBUS_GPIO & 63), .mode = GPIO_MODE_OUTPUT};
        ESP_ERROR_CHECK(gpio_config(&cfg));
        ESP_ERROR_CHECK(gpio_set_level(CONFIG_SWITCH_VBUS_GPIO, CONFIG_SWITCH_VBUS_ACTIVE_LEVEL));
        vTaskDelay(pdMS_TO_TICKS(100));
    }
    else
        ESP_LOGW(TAG, "VBUS GPIO unset: board must supply host 5V itself");
    usb_host_config_t cfg = {.skip_phy_setup = false, .intr_flags = ESP_INTR_FLAG_LOWMED};
    ESP_ERROR_CHECK(usb_host_install(&cfg));
    if (xTaskCreate(library_task, "usb_library", 4096, NULL, 4, NULL) != pdPASS)
        return ESP_ERR_NO_MEM;
    if (xTaskCreate(connect_task, "usb_connect", 6144, NULL, 3, NULL) != pdPASS)
        return ESP_ERR_NO_MEM;
    hid_host_driver_config_t hid_cfg = {
        .create_background_task = true,
        .task_priority = 5,
        .stack_size = 6144,
        .core_id = tskNO_AFFINITY,
        .callback = connect_callback,
    };
    return hid_host_install(&hid_cfg);
}

bool usb_input_receive(usb_input_event_t *event)
{
    return xQueueReceive(input_queue, event, 0) == pdTRUE;
}
bool usb_input_overflowed(void)
{
    return atomic_exchange(&overflow, false);
}
void usb_input_status(void)
{
    ESP_LOGI(TAG, "Active HID interfaces=%u decoded reports=%u queue=%u", atomic_load(&active_interfaces),
             atomic_load(&report_count), (unsigned)uxQueueMessagesWaiting(input_queue));
}

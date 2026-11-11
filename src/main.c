#include "ble_switch.h"
#include "driver/gpio.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "nvs_flash.h"
#include "sdkconfig.h"
#include "usb_input.h"
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static const char *TAG = "switch";
static hid_input_t sources[USB_INPUT_SOURCES];
static bool armed;
static uint8_t last_keyboard[8], last_mouse_buttons;
static uint16_t last_consumer;

static void help(void)
{
    puts("\nBT Switch: n=next slot, 1..N=select slot, p=open pairing on empty "
         "slot,");
    puts("s=status, h=help, X=FORGET selected slot (also remove pairing on "
         "host).");
    puts("t=send neutral HID probe to input target (no keys/movement).");
    puts("Button (if configured): short press=next, hold 3s=open pairing.\n");
}

static hid_input_t aggregate(void)
{
    hid_input_t out = {0};
    for (unsigned i = 0; i < USB_INPUT_SOURCES; ++i)
    {
        out.buttons |= sources[i].buttons;
        for (unsigned k = 0; k < HID_KEYS_BYTES; ++k)
            out.keys[k] |= sources[i].keys[k];
        if (!out.consumer)
            out.consumer = sources[i].consumer;
    }
    return out;
}

static bool neutral(const hid_input_t *s)
{
    if (s->buttons || s->consumer)
        return false;
    for (unsigned i = 0; i < HID_KEYS_BYTES; ++i)
        if (s->keys[i])
            return false;
    return true;
}

static void disarm(void)
{
    armed = false;
    memset(last_keyboard, 0, sizeof(last_keyboard));
    last_mouse_buttons = 0;
    last_consumer = 0;
}

static int32_t clamp(int32_t value, int32_t limit)
{
    return value > limit ? limit : value < -limit ? -limit : value;
}

static void forward(const hid_input_t *motion)
{
    if (!ble_switch_ready())
    {
        disarm();
        return;
    }
    hid_input_t state = aggregate();
    if (!armed)
    {
        // Never carry Ctrl/Alt, a held mouse button, or a media key across hosts.
        if (!neutral(&state))
            return;
        ble_switch_release();
        armed = true;
    }
    uint8_t keyboard[8] = {0};
    unsigned count = 0;
    for (unsigned key = 0; key < 256; ++key)
    {
        if (!(state.keys[key / 8] & (1u << (key % 8))))
            continue;
        if (key >= 0xe0 && key <= 0xe7)
            keyboard[0] |= 1u << (key - 0xe0);
        else if (key >= 4 && key < 0xe0)
        {
            if (count < 6)
                keyboard[2 + count] = key;
            ++count;
        }
        else if (key >= 1 && key <= 3)
            count = 7;
    }
    if (count > 6)
        memset(keyboard + 2, 1, 6); // HID ErrorRollOver
    if (memcmp(keyboard, last_keyboard, sizeof(keyboard)))
    {
        if (ble_switch_send(1, keyboard, sizeof(keyboard)))
            memcpy(last_keyboard, keyboard, sizeof(keyboard));
    }
    if (state.consumer != last_consumer)
    {
        uint8_t data[] = {state.consumer & 0xff, state.consumer >> 8};
        if (ble_switch_send(3, data, sizeof(data)))
            last_consumer = state.consumer;
    }
    int32_t x = motion ? motion->x : 0, y = motion ? motion->y : 0;
    int32_t wheel = motion ? motion->wheel : 0, pan = motion ? motion->pan : 0;
    if (x || y || wheel || pan || state.buttons != last_mouse_buttons)
    {
        // Bound each delta. Normal receiver reports fit without saturation.
        int16_t dx = clamp(x, INT16_MAX), dy = clamp(y, INT16_MAX);
        uint8_t data[] = {state.buttons,           dx & 0xff,
                          (uint16_t)dx >> 8,       dy & 0xff,
                          (uint16_t)dy >> 8,       (uint8_t)clamp(wheel, 127),
                          (uint8_t)clamp(pan, 127)};
        if (ble_switch_send(2, data, sizeof(data)))
            last_mouse_buttons = state.buttons;
    }
}

static void command(char c)
{
    if (c == 'n' || (c >= '1' && c < '1' + CONFIG_SWITCH_SLOT_COUNT))
    {
        unsigned slot = c == 'n' ? (ble_switch_slot() + 1) % CONFIG_SWITCH_SLOT_COUNT : c - '1';
        ble_switch_select(slot);
        disarm();
    }
    else if (c == 'p')
    {
        ble_switch_pair();
        disarm();
    }
    else if (c == 'X')
    {
        ble_switch_forget();
        disarm();
    }
    else if (c == 's')
    {
        ble_switch_status();
        usb_input_status();
    }
    else if (c == 't')
    {
        ble_switch_probe();
        disarm();
    }
    else if (c == 'h' || c == '?')
        help();
}

static void button_tick(void)
{
    if (CONFIG_SWITCH_BUTTON_GPIO < 0)
        return;
    static int raw_previous = 1, stable = 1;
    static int64_t changed_at, pressed_at;
    static bool long_fired;
    int64_t now = esp_timer_get_time();
    int raw = gpio_get_level(CONFIG_SWITCH_BUTTON_GPIO);
    if (raw != raw_previous)
    {
        raw_previous = raw;
        changed_at = now;
    }
    if (raw != stable && now - changed_at >= 30000)
    {
        stable = raw;
        if (!stable)
        {
            pressed_at = now;
            long_fired = false;
        }
        else if (!long_fired)
            command('n');
    }
    if (!stable && !long_fired && now - pressed_at >= 3000000)
    {
        long_fired = true;
        command('p');
    }
}

void app_main(void)
{
    // Do not silently erase Bluetooth pairings if NVS is corrupt/full.
    ESP_ERROR_CHECK(nvs_flash_init());
    if (CONFIG_SWITCH_BUTTON_GPIO >= 0)
    {
        if (!GPIO_IS_VALID_GPIO(CONFIG_SWITCH_BUTTON_GPIO) || CONFIG_SWITCH_BUTTON_GPIO == 19 ||
            CONFIG_SWITCH_BUTTON_GPIO == 20 || CONFIG_SWITCH_BUTTON_GPIO == CONFIG_SWITCH_VBUS_GPIO)
        {
            ESP_LOGE(TAG, "Invalid/conflicting button GPIO");
            return;
        }
        gpio_config_t cfg = {
            .pin_bit_mask = 1ULL << (CONFIG_SWITCH_BUTTON_GPIO & 63),
            .mode = GPIO_MODE_INPUT,
            .pull_up_en = GPIO_PULLUP_ENABLE,
        };
        ESP_ERROR_CHECK(gpio_config(&cfg));
    }
    else
        ESP_LOGW(TAG, "Button disabled; use serial n / 1..N to switch");
    setvbuf(stdin, NULL, _IONBF, 0);
    int flags = fcntl(STDIN_FILENO, F_GETFL, 0);
    if (flags >= 0)
        fcntl(STDIN_FILENO, F_SETFL, flags | O_NONBLOCK);
    ESP_ERROR_CHECK(ble_switch_init());
    ESP_ERROR_CHECK(usb_input_init());
    help();
    for (;;)
    {
        ble_switch_tick();
        button_tick();
        char c;
        for (unsigned i = 0; i < 16 && read(STDIN_FILENO, &c, 1) > 0; ++i)
            command(c);
        if (usb_input_overflowed())
        {
            usb_input_event_t discarded;
            while (usb_input_receive(&discarded))
            {
            }
            ble_switch_release();
            memset(sources, 0, sizeof(sources));
            disarm();
            ESP_LOGW(TAG, "Input queue overflow: released all keys; discarded queued input");
        }
        usb_input_event_t e;
        for (unsigned i = 0; i < 32 && usb_input_receive(&e); ++i)
        {
            if (e.source >= USB_INPUT_SOURCES)
                continue;
            sources[e.source] = e.input;
            forward(&e.input);
        }
        forward(NULL);
        vTaskDelay(pdMS_TO_TICKS(10));
    }
}

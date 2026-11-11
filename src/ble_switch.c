#include "ble_switch.h"
#include "esp_bt.h"
#include "esp_bt_main.h"
#include "esp_gap_ble_api.h"
#include "esp_gatts_api.h"
#include "esp_log.h"
#include "esp_system.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "hid_profile.h"
#include "hid_router.h"
#include "nvs.h"
#include "sdkconfig.h"
#include <inttypes.h>
#include <string.h>

#define SLOT_COUNT CONFIG_SWITCH_SLOT_COUNT
_Static_assert(SLOT_COUNT <= HID_ROUTER_MAX_PEERS, "Too many peer slots");
_Static_assert(CONFIG_BT_ACL_CONNECTIONS >= SLOT_COUNT, "Increase BT_ACL_CONNECTIONS");
_Static_assert(CONFIG_BT_CTRL_BLE_MAX_ACT >= SLOT_COUNT + 1, "Reserve one controller activity for advertising");
static const char *TAG = "ble_switch";

// Preserve the original NVS layout and public Bluetooth identity. No key or
// existing slot is discarded by the migration from the single-link firmware.
typedef struct
{
    uint8_t used, address[6], type;
} slot_t;
typedef struct
{
    uint32_t version;
    uint8_t selected;
    slot_t slots[SLOT_COUNT];
} storage_t;
typedef struct
{
    bool used, closing, announced;
    uint16_t connection, mtu;
    uint8_t address[6], protocol, leds, cccd;
    int candidate; // pairing target captured at connection time, not active input
    int64_t connected_at;
    int64_t encryption_requested_at;
    bool security_request_pending;
    uint32_t sent[HID_ROUTER_REPORTS];
} link_t;
static storage_t store;
static uint8_t saved_cccd[SLOT_COUNT];
static nvs_handle_t nvs;
static hid_router_t router;
static link_t links[HID_ROUTER_MAX_PEERS];
static esp_gatt_if_t gatt_if = ESP_GATT_IF_NONE;
static uint16_t battery_handles[B_COUNT], info_handles[I_COUNT], hid_handles[H_COUNT];
static unsigned started_services;
static bool data_ready, services_ready;
static int pairing_slot = -1, forgetting_slot = -1;
static bool removing_bond;
static int security_owner = -1;
static int64_t pairing_until;

// Every mutable ATT operation is copied to the application task. No callback
// retains stack-owned pointers (notably write.value and add_attr_tab.handles).
typedef enum
{
    E_REGISTER,
    E_TABLE,
    E_SERVICE_STARTED,
    E_CONNECT,
    E_DISCONNECT,
    E_READ,
    E_WRITE,
    E_MTU,
    E_CONGESTION,
    E_NOTIFY_ERROR,
    E_ADV_DATA,
    E_ADV_START,
    E_ADV_STOP,
    E_WHITELIST,
    E_AUTH,
    E_SECURITY,
    E_NUMERIC,
    E_BOND_REMOVED,
} event_kind_t;
typedef struct
{
    event_kind_t kind;
    uint16_t connection, handle, offset, length, mtu;
    uint32_t transaction;
    int status;
    uint8_t address[6], address_type, instance, count;
    bool need_response, prepared, flag;
    uint8_t value[8];
    uint16_t handles[H_COUNT];
} event_t;
static QueueHandle_t events;

typedef enum
{
    A_IDLE,
    A_STOPPING,
    A_CLEARING,
    A_ADDING,
    A_STARTING,
    A_RUNNING
} adv_state_t;
static adv_state_t adv_state;
static bool adv_dirty = true;
static unsigned whitelist_next;
static int64_t adv_retry_at;
static esp_ble_adv_params_t adv_params = {
    .adv_int_min = 0x30,
    .adv_int_max = 0x60,
    .adv_type = ADV_TYPE_IND,
    .own_addr_type = BLE_ADDR_TYPE_PUBLIC,
    .channel_map = ADV_CHNL_ALL,
};

static void post(event_t *e)
{
    if (xQueueSend(events, e, 0) != pdTRUE)
    {
        ESP_LOGE(TAG, "BLE control queue overflow; restarting safely");
        esp_restart();
    }
}

static void gap_callback(esp_gap_ble_cb_event_t kind, esp_ble_gap_cb_param_t *p)
{
    event_t e = {0};
    switch (kind)
    {
    case ESP_GAP_BLE_ADV_DATA_SET_COMPLETE_EVT:
        e.kind = E_ADV_DATA;
        e.status = p->adv_data_cmpl.status;
        break;
    case ESP_GAP_BLE_ADV_START_COMPLETE_EVT:
        e.kind = E_ADV_START;
        e.status = p->adv_start_cmpl.status;
        break;
    case ESP_GAP_BLE_ADV_STOP_COMPLETE_EVT:
        e.kind = E_ADV_STOP;
        e.status = p->adv_stop_cmpl.status;
        break;
    case ESP_GAP_BLE_UPDATE_WHITELIST_COMPLETE_EVT:
        e.kind = E_WHITELIST;
        e.status = p->update_whitelist_cmpl.status;
        break;
    case ESP_GAP_BLE_AUTH_CMPL_EVT:
        e.kind = E_AUTH;
        e.flag = p->ble_security.auth_cmpl.success;
        e.status = p->ble_security.auth_cmpl.fail_reason;
        e.address_type = p->ble_security.auth_cmpl.addr_type;
        memcpy(e.address, p->ble_security.auth_cmpl.bd_addr, 6);
        break;
    case ESP_GAP_BLE_SEC_REQ_EVT:
        e.kind = E_SECURITY;
        memcpy(e.address, p->ble_security.ble_req.bd_addr, 6);
        break;
    case ESP_GAP_BLE_NC_REQ_EVT:
        e.kind = E_NUMERIC;
        memcpy(e.address, p->ble_security.key_notif.bd_addr, 6);
        break;
    case ESP_GAP_BLE_REMOVE_BOND_DEV_COMPLETE_EVT:
        e.kind = E_BOND_REMOVED;
        e.status = p->remove_bond_dev_cmpl.status;
        memcpy(e.address, p->remove_bond_dev_cmpl.bd_addr, 6);
        break;
    default:
        return;
    }
    post(&e);
}

static void gatts_callback(esp_gatts_cb_event_t kind, esp_gatt_if_t interface, esp_ble_gatts_cb_param_t *p)
{
    event_t e = {0};
    switch (kind)
    {
    case ESP_GATTS_REG_EVT:
        e.kind = E_REGISTER;
        e.status = p->reg.status;
        e.handle = interface;
        break;
    case ESP_GATTS_CREAT_ATTR_TAB_EVT:
        e.kind = E_TABLE;
        e.status = p->add_attr_tab.status;
        e.instance = p->add_attr_tab.svc_inst_id;
        if (p->add_attr_tab.num_handle > H_COUNT)
        {
            ESP_LOGE(TAG, "Too many GATT handles");
            return;
        }
        e.count = p->add_attr_tab.num_handle;
        if (e.status == ESP_GATT_OK)
            memcpy(e.handles, p->add_attr_tab.handles, e.count * sizeof(uint16_t));
        break;
    case ESP_GATTS_START_EVT:
        e.kind = E_SERVICE_STARTED;
        e.status = p->start.status;
        break;
    case ESP_GATTS_CONNECT_EVT:
        e.kind = E_CONNECT;
        e.connection = p->connect.conn_id;
        memcpy(e.address, p->connect.remote_bda, 6);
        break;
    case ESP_GATTS_DISCONNECT_EVT:
        e.kind = E_DISCONNECT;
        e.connection = p->disconnect.conn_id;
        e.status = p->disconnect.reason;
        memcpy(e.address, p->disconnect.remote_bda, 6);
        break;
    case ESP_GATTS_READ_EVT:
        e.kind = E_READ;
        e.connection = p->read.conn_id;
        e.transaction = p->read.trans_id;
        e.handle = p->read.handle;
        e.offset = p->read.offset;
        break;
    case ESP_GATTS_WRITE_EVT:
        e.kind = E_WRITE;
        e.connection = p->write.conn_id;
        e.transaction = p->write.trans_id;
        e.handle = p->write.handle;
        e.offset = p->write.offset;
        e.length = p->write.len;
        e.need_response = p->write.need_rsp;
        e.prepared = p->write.is_prep;
        if (e.length <= sizeof(e.value))
            memcpy(e.value, p->write.value, e.length);
        break;
    case ESP_GATTS_EXEC_WRITE_EVT:
        // None of our writable values is longer than two bytes. Prepared
        // transactions are never accepted, so an execute cannot commit state.
        esp_ble_gatts_send_response(interface, p->exec_write.conn_id, p->exec_write.trans_id,
                                    ESP_GATT_REQ_NOT_SUPPORTED, NULL);
        return;
    case ESP_GATTS_MTU_EVT:
        e.kind = E_MTU;
        e.connection = p->mtu.conn_id;
        e.mtu = p->mtu.mtu;
        break;
    case ESP_GATTS_CONGEST_EVT:
        e.kind = E_CONGESTION;
        e.connection = p->congest.conn_id;
        e.flag = p->congest.congested;
        break;
    case ESP_GATTS_CONF_EVT:
        if (p->conf.status == ESP_GATT_OK)
            return;
        e.kind = E_NOTIFY_ERROR;
        e.connection = p->conf.conn_id;
        e.status = p->conf.status;
        break;
    default:
        return;
    }
    post(&e);
}

// Static attributes use stack-managed read responses. All connection-specific
// CCCDs, protocol/control state and report values use application responses.
static const uint16_t u_service = 0x2800, u_include = 0x2802, u_char = 0x2803, u_ccc = 0x2902;
static const uint16_t u_battery = 0x180f, u_info = 0x180a, u_hid = 0x1812;
static const uint16_t u_level = 0x2a19, u_format = 0x2904, u_pnp = 0x2a50, u_maker = 0x2a29, u_serial = 0x2a25;
static const uint16_t u_hid_info = 0x2a4a, u_control = 0x2a4c, u_protocol = 0x2a4e, u_map = 0x2a4b, u_external = 0x2907;
static const uint16_t u_report = 0x2a4d, u_reference = 0x2908, u_boot_key = 0x2a22, u_boot_led = 0x2a32,
                      u_boot_mouse = 0x2a33;
static const uint8_t prop_read = ESP_GATT_CHAR_PROP_BIT_READ;
static const uint8_t prop_notify = ESP_GATT_CHAR_PROP_BIT_READ | ESP_GATT_CHAR_PROP_BIT_NOTIFY;
static const uint8_t prop_write = ESP_GATT_CHAR_PROP_BIT_WRITE_NR;
static const uint8_t prop_rw = ESP_GATT_CHAR_PROP_BIT_READ | ESP_GATT_CHAR_PROP_BIT_WRITE_NR;
static const uint8_t prop_output =
    ESP_GATT_CHAR_PROP_BIT_READ | ESP_GATT_CHAR_PROP_BIT_WRITE | ESP_GATT_CHAR_PROP_BIT_WRITE_NR;
static const uint8_t battery_level = 100, hid_info[] = {0x11, 0x01, 0x00, 0x03};
static const uint8_t pnp[] = {2, 0xff, 0xff, 1, 0, 0, 1};
static const uint8_t key_ref[] = {1, 1}, led_ref[] = {1, 2}, mouse_ref[] = {2, 1}, consumer_ref[] = {3, 1};
static const char maker[] = "DIY", serial_number[] = "0001";
static esp_gatts_incl_svc_desc_t battery_include;
static esp_gatts_attr_db_t battery_db[B_COUNT], info_db[I_COUNT], hid_db[H_COUNT];

static void attr(esp_gatts_attr_db_t *db, unsigned index, const uint16_t *uuid, unsigned permission, unsigned maximum,
                 unsigned length, const void *value, bool dynamic)
{
    db[index] = (esp_gatts_attr_db_t){
        .attr_control = {.auto_rsp = dynamic ? ESP_GATT_RSP_BY_APP : ESP_GATT_AUTO_RSP},
        .att_desc = {.uuid_length = ESP_UUID_LEN_16,
                     .uuid_p = (uint8_t *)uuid,
                     .perm = permission,
                     .max_length = maximum,
                     .length = length,
                     .value = (uint8_t *)value},
    };
}

static void build_tables(void)
{
#define FIXED(db, i, uuid, val) attr(db, i, uuid, ESP_GATT_PERM_READ, sizeof(val), sizeof(val), &(val), false)
#define DECL(db, i, prop) FIXED(db, i, &u_char, prop)
#define DYNAMIC(db, i, uuid, size)                                                                                     \
    attr(db, i, uuid, ESP_GATT_PERM_READ_ENCRYPTED | ESP_GATT_PERM_WRITE_ENCRYPTED, size, 0, NULL, true)
    FIXED(battery_db, B_SERVICE, &u_service, u_battery);
    DECL(battery_db, B_CHAR, prop_notify);
    FIXED(battery_db, B_LEVEL, &u_level, battery_level);
    DYNAMIC(battery_db, B_CCC, &u_ccc, 2);
    // Same attribute footprint as esp_hid, to retain cached handles.
    attr(battery_db, B_FORMAT, &u_format, ESP_GATT_PERM_READ, 7, 0, NULL, false);
    FIXED(info_db, I_SERVICE, &u_service, u_info);
    DECL(info_db, I_PNP_CHAR, prop_read);
    FIXED(info_db, I_PNP, &u_pnp, pnp);
    DECL(info_db, I_MAKER_CHAR, prop_read);
    attr(info_db, I_MAKER, &u_maker, ESP_GATT_PERM_READ, 3, 3, maker, false);
    DECL(info_db, I_SERIAL_CHAR, prop_read);
    attr(info_db, I_SERIAL, &u_serial, ESP_GATT_PERM_READ, 4, 4, serial_number, false);
    FIXED(hid_db, H_SERVICE, &u_service, u_hid);
    FIXED(hid_db, H_BATTERY_INCLUDE, &u_include, battery_include);
    DECL(hid_db, H_INFO_CHAR, prop_read);
    FIXED(hid_db, H_INFO, &u_hid_info, hid_info);
    DECL(hid_db, H_CONTROL_CHAR, prop_write);
    DYNAMIC(hid_db, H_CONTROL, &u_control, 1);
    DECL(hid_db, H_PROTOCOL_CHAR, prop_rw);
    DYNAMIC(hid_db, H_PROTOCOL, &u_protocol, 1);
    DECL(hid_db, H_MAP_CHAR, prop_read);
    FIXED(hid_db, H_MAP, &u_map, report_map);
    FIXED(hid_db, H_MAP_EXTERNAL, &u_external, u_level);
    DECL(hid_db, H_KEY_CHAR, prop_notify);
    DYNAMIC(hid_db, H_KEY, &u_report, 8);
    DYNAMIC(hid_db, H_KEY_CCC, &u_ccc, 2);
    FIXED(hid_db, H_KEY_REF, &u_reference, key_ref);
    DECL(hid_db, H_BOOT_KEY_CHAR, prop_notify);
    DYNAMIC(hid_db, H_BOOT_KEY, &u_boot_key, 8);
    DYNAMIC(hid_db, H_BOOT_KEY_CCC, &u_ccc, 2);
    DECL(hid_db, H_LED_CHAR, prop_output);
    DYNAMIC(hid_db, H_LED, &u_report, 1);
    FIXED(hid_db, H_LED_REF, &u_reference, led_ref);
    DECL(hid_db, H_BOOT_LED_CHAR, prop_output);
    DYNAMIC(hid_db, H_BOOT_LED, &u_boot_led, 1);
    DECL(hid_db, H_MOUSE_CHAR, prop_notify);
    DYNAMIC(hid_db, H_MOUSE, &u_report, 7);
    DYNAMIC(hid_db, H_MOUSE_CCC, &u_ccc, 2);
    FIXED(hid_db, H_MOUSE_REF, &u_reference, mouse_ref);
    DECL(hid_db, H_BOOT_MOUSE_CHAR, prop_notify);
    DYNAMIC(hid_db, H_BOOT_MOUSE, &u_boot_mouse, 3);
    DYNAMIC(hid_db, H_BOOT_MOUSE_CCC, &u_ccc, 2);
    DECL(hid_db, H_CONSUMER_CHAR, prop_notify);
    DYNAMIC(hid_db, H_CONSUMER, &u_report, 2);
    DYNAMIC(hid_db, H_CONSUMER_CCC, &u_ccc, 2);
    FIXED(hid_db, H_CONSUMER_REF, &u_reference, consumer_ref);
#undef FIXED
#undef DECL
#undef DYNAMIC
}

static void save_slots(void)
{
    store.version = 1;
    store.selected = router.selected;
    ESP_ERROR_CHECK(nvs_set_blob(nvs, "slots", &store, sizeof(store)));
    ESP_ERROR_CHECK(nvs_commit(nvs));
}

static void save_subscriptions(void)
{
    ESP_ERROR_CHECK(nvs_set_blob(nvs, "cccd", saved_cccd, sizeof(saved_cccd)));
    ESP_ERROR_CHECK(nvs_commit(nvs));
}

static link_t *by_id(uint16_t id)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (links[i].used && links[i].connection == id)
            return &links[i];
    return NULL;
}

static link_t *by_address(const uint8_t *address)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (links[i].used && !memcmp(links[i].address, address, 6))
            return &links[i];
    return NULL;
}

static int stored_slot(const uint8_t *address)
{
    for (unsigned i = 0; i < SLOT_COUNT; ++i)
        if (store.slots[i].used && !memcmp(store.slots[i].address, address, 6))
            return i;
    return -1;
}

static unsigned link_count(void)
{
    unsigned count = 0;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        count += links[i].used;
    return count;
}

static bool pairing_open(void)
{
    return pairing_slot >= 0 && !store.slots[pairing_slot].used && forgetting_slot < 0 &&
           esp_timer_get_time() < pairing_until;
}

static bool unknown_pending(void)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (links[i].used && !links[i].closing && links[i].candidate >= 0)
            return true;
    return false;
}

static void close_link(link_t *l)
{
    if (!l || l->closing)
        return;
    hid_peer_t *peer = hid_router_peer(&router, l->connection);
    if (peer)
        peer->encrypted = false;
    esp_err_t err = esp_ble_gap_disconnect(l->address);
    if (err == ESP_OK)
        l->closing = true;
    else
        ESP_LOGW(TAG, "Disconnect conn=%u: %s", l->connection, esp_err_to_name(err));
}

static uint8_t notification_mask(const link_t *l)
{
    if (l->protocol)
        return l->cccd & 7;
    return ((l->cccd & CCC_BOOT_KEY) ? 1 : 0) | ((l->cccd & CCC_BOOT_MOUSE) ? 2 : 0);
}

static void subscriptions(link_t *l)
{
    hid_router_subscribe(&router, l->connection, notification_mask(l));
    hid_peer_t *p = hid_router_peer(&router, l->connection);
    if (p && p->encrypted && p->slot >= 0 && saved_cccd[p->slot] != l->cccd)
    {
        saved_cccd[p->slot] = l->cccd;
        save_subscriptions();
    }
}

static int8_t boot_delta(int value)
{
    return value > 127 ? 127 : value < -127 ? -127 : value;
}

static bool notify(void *context, uint16_t connection, uint8_t report, const uint8_t *data, size_t size)
{
    (void)context;
    link_t *l = by_id(connection);
    if (!l || l->closing)
        return false;
    uint16_t handle = 0;
    uint8_t boot[3];
    if (l->protocol)
    {
        handle = hid_handles[report == 1 ? H_KEY : report == 2 ? H_MOUSE : H_CONSUMER];
    }
    else if (report == 1)
        handle = hid_handles[H_BOOT_KEY];
    else if (report == 2)
    {
        handle = hid_handles[H_BOOT_MOUSE];
        boot[0] = data[0] & 7;
        boot[1] = boot_delta((int16_t)((uint16_t)data[1] | (uint16_t)data[2] << 8));
        boot[2] = boot_delta((int16_t)((uint16_t)data[3] | (uint16_t)data[4] << 8));
        data = boot;
        size = sizeof(boot);
    }
    else
        return false;
    bool success = esp_ble_gatts_send_indicate(gatt_if, connection, handle, size, (uint8_t *)data, false) == ESP_OK;
    if (success)
        ++l->sent[report - 1];
    return success;
}

static uint8_t ccc_bit(uint16_t handle)
{
    if (handle == hid_handles[H_KEY_CCC])
        return CCC_KEY;
    if (handle == hid_handles[H_MOUSE_CCC])
        return CCC_MOUSE;
    if (handle == hid_handles[H_CONSUMER_CCC])
        return CCC_CONSUMER;
    if (handle == hid_handles[H_BOOT_KEY_CCC])
        return CCC_BOOT_KEY;
    if (handle == hid_handles[H_BOOT_MOUSE_CCC])
        return CCC_BOOT_MOUSE;
    if (handle == battery_handles[B_CCC])
        return CCC_BATTERY;
    return 0;
}

static bool dynamic_handle(uint16_t handle)
{
    return ccc_bit(handle) || handle == hid_handles[H_PROTOCOL] || handle == hid_handles[H_CONTROL] ||
           handle == hid_handles[H_LED] || handle == hid_handles[H_BOOT_LED] || handle == hid_handles[H_KEY] ||
           handle == hid_handles[H_BOOT_KEY] || handle == hid_handles[H_MOUSE] || handle == hid_handles[H_BOOT_MOUSE] ||
           handle == hid_handles[H_CONSUMER];
}

static void authenticated_event(const event_t *e);

static void encrypted_att_proof(link_t *l, hid_peer_t *peer)
{
    // All dynamic attributes require encryption in the stack's permission
    // table. Its security checks run BEFORE dispatching an app read/write.
    // A bonded reconnect can already be encrypted by the central before our
    // SetEncryption request, in which case Bluedroid emits no new AUTH_CMPL.
    // Receiving this ATT request is authoritative proof, not a timer heuristic.
    if (!l || !peer || peer->encrypted || l->closing)
        return;
    int slot = stored_slot(l->address);
    if (slot < 0)
        return; // newly paired devices still require AUTH_CMPL + admission
    event_t e = {.kind = E_AUTH, .flag = true, .address_type = store.slots[slot].type};
    memcpy(e.address, l->address, 6);
    authenticated_event(&e);
}

static void read_value(const event_t *e)
{
    link_t *l = by_id(e->connection);
    hid_peer_t *peer = hid_router_peer(&router, e->connection);
    esp_gatt_status_t status = ESP_GATT_OK;
    esp_gatt_rsp_t response = {0};
    response.attr_value.handle = e->handle;
    response.attr_value.offset = e->offset;
    uint8_t value[8] = {0};
    unsigned length = 0;
    encrypted_att_proof(l, peer);
    if (!l || !peer)
        status = ESP_GATT_INVALID_HANDLE;
    else if (!peer->encrypted)
        status = ESP_GATT_INSUF_ENCRYPTION;
    else if (ccc_bit(e->handle))
    {
        value[0] = (l->cccd & ccc_bit(e->handle)) ? 1 : 0;
        length = 2;
    }
    else if (e->handle == hid_handles[H_PROTOCOL])
    {
        value[0] = l->protocol;
        length = 1;
    }
    else if (e->handle == hid_handles[H_CONTROL])
    {
        value[0] = peer->suspended ? 0 : 1;
        length = 1;
    }
    else if (e->handle == hid_handles[H_LED] || e->handle == hid_handles[H_BOOT_LED])
    {
        value[0] = l->leds;
        length = 1;
    }
    else
    {
        uint8_t report = e->handle == hid_handles[H_KEY] || e->handle == hid_handles[H_BOOT_KEY]       ? 1
                         : e->handle == hid_handles[H_MOUSE] || e->handle == hid_handles[H_BOOT_MOUSE] ? 2
                         : e->handle == hid_handles[H_CONSUMER]                                        ? 3
                                                                                                       : 0;
        if (!report)
            status = ESP_GATT_READ_NOT_PERMIT;
        else
        {
            length = hid_router_report_size(report);
            memcpy(value, peer->values[report - 1], length);
            if (e->handle == hid_handles[H_BOOT_MOUSE])
            {
                value[0] &= 7;
                value[1] = value[2] = 0;
                length = 3;
            }
        }
    }
    if (status == ESP_GATT_OK)
    {
        if (e->offset > length)
            status = ESP_GATT_INVALID_OFFSET;
        else
        {
            response.attr_value.len = length - e->offset;
            memcpy(response.attr_value.value, value + e->offset, response.attr_value.len);
        }
    }
    esp_ble_gatts_send_response(gatt_if, e->connection, e->transaction, status, &response);
}

static void write_value(const event_t *e)
{
    link_t *l = by_id(e->connection);
    hid_peer_t *peer = hid_router_peer(&router, e->connection);
    encrypted_att_proof(l, peer);
    esp_gatt_status_t status = ESP_GATT_OK;
    if (!l || !peer)
        status = ESP_GATT_INVALID_HANDLE;
    else if (e->prepared)
        status = ESP_GATT_REQ_NOT_SUPPORTED;
    else if (e->offset)
        status = ESP_GATT_INVALID_OFFSET;
    else if (!peer->encrypted || peer->slot < 0)
        status = ESP_GATT_INSUF_ENCRYPTION;
    else if (ccc_bit(e->handle))
    {
        if (e->length != 2)
            status = ESP_GATT_INVALID_ATTR_LEN;
        else if (e->value[1] || e->value[0] > 1)
            status = ESP_GATT_CCC_CFG_ERR;
        else
        {
            uint8_t bit = ccc_bit(e->handle);
            if (e->value[0])
                l->cccd |= bit;
            else
                l->cccd &= ~bit;
            subscriptions(l);
            ESP_LOGI(TAG, "Slot %d subscriptions=0x%02x conn=%u", peer->slot + 1, l->cccd, l->connection);
        }
    }
    else if (e->handle == hid_handles[H_PROTOCOL])
    {
        if (e->length != 1)
            status = ESP_GATT_INVALID_ATTR_LEN;
        else if (e->value[0] > 1)
            status = ESP_GATT_OUT_OF_RANGE;
        else
        {
            l->protocol = e->value[0];
            subscriptions(l);
            peer->releases |= peer->subscriptions;
        }
    }
    else if (e->handle == hid_handles[H_CONTROL])
    {
        if (e->length != 1)
            status = ESP_GATT_INVALID_ATTR_LEN;
        else if (e->value[0] > 1)
            status = ESP_GATT_OUT_OF_RANGE;
        else
        {
            peer->suspended = !e->value[0];
            peer->releases |= peer->subscriptions;
        }
    }
    else if (e->handle == hid_handles[H_LED] || e->handle == hid_handles[H_BOOT_LED])
    {
        if (e->length != 1)
            status = ESP_GATT_INVALID_ATTR_LEN;
        else
            l->leds = e->value[0];
    }
    else
        status = ESP_GATT_WRITE_NOT_PERMIT;
    if (e->need_response)
        esp_ble_gatts_send_response(gatt_if, e->connection, e->transaction, status, NULL);
}

// Adv/whitelist changes never tear down live links. All bonded hosts, not just
// the selected input target, are eligible to reconnect in the controller.
static void advertising_failed(const char *operation, int status)
{
    ESP_LOGE(TAG, "Advertising %s failed: %d", operation, status);
    adv_state = A_IDLE;
    adv_dirty = true;
    adv_retry_at = esp_timer_get_time() + 1000000;
}

static void advertise(void)
{
    if (link_count() >= SLOT_COUNT)
    {
        adv_state = A_IDLE;
        return;
    }
    adv_params.adv_filter_policy =
        pairing_open() && !unknown_pending() ? ADV_FILTER_ALLOW_SCAN_ANY_CON_ANY : ADV_FILTER_ALLOW_SCAN_ANY_CON_WLST;
    esp_err_t err = esp_ble_gap_start_advertising(&adv_params);
    if (err == ESP_OK)
        adv_state = A_STARTING;
    else
        advertising_failed("start", err);
}

static void whitelist_add_next(void)
{
    while (whitelist_next < SLOT_COUNT)
    {
        unsigned i = whitelist_next++;
        slot_t *s = &store.slots[i];
        if (!s->used || forgetting_slot == (int)i)
            continue;
        // The stack may translate the stored pseudo-address to its bonded
        // identity address; never let that mutate our persisted slot record.
        esp_bd_addr_t address;
        memcpy(address, s->address, 6);
        esp_err_t err = esp_ble_gap_update_whitelist(true, address,
                                                     (s->type & 1) ? BLE_WL_ADDR_TYPE_RANDOM : BLE_WL_ADDR_TYPE_PUBLIC);
        if (err == ESP_OK)
            adv_state = A_ADDING;
        else
            advertising_failed("whitelist add", err);
        return;
    }
    advertise();
}

static void advertising_tick(void)
{
    if (pairing_slot >= 0 && !pairing_open())
    {
        pairing_slot = -1;
        adv_dirty = true;
    }
    if (!services_ready || !data_ready || esp_timer_get_time() < adv_retry_at)
        return;
    if (adv_state == A_RUNNING && adv_dirty)
    {
        esp_err_t err = esp_ble_gap_stop_advertising();
        if (err == ESP_OK)
            adv_state = A_STOPPING;
        else
            advertising_failed("stop", err);
    }
    else if (adv_state == A_IDLE && adv_dirty)
    {
        adv_dirty = false;
        if (link_count() >= SLOT_COUNT)
            return;
        esp_err_t err = esp_ble_gap_clear_whitelist();
        if (err == ESP_OK)
        {
            adv_state = A_CLEARING;
            whitelist_next = 0;
        }
        else
            advertising_failed("whitelist clear", err);
    }
}

static void open_pairing(unsigned slot)
{
    if (slot >= SLOT_COUNT || store.slots[slot].used || forgetting_slot >= 0)
        return;
    pairing_slot = slot;
    pairing_until = esp_timer_get_time() + (int64_t)CONFIG_SWITCH_PAIRING_SECONDS * 1000000;
    adv_dirty = true;
    ESP_LOGI(TAG, "Pairing window open for slot %u; existing links remain online", slot + 1);
}

static void connected_event(const event_t *e)
{
    if (by_id(e->connection))
        return;
    // A successful connect stops the advertising instance, not other links.
    // Bluedroid automatically resumes connectable advertising after a link is
    // established. Explicitly stop and await its completion before touching
    // the controller whitelist, including after the second/third connection.
    if (adv_state == A_IDLE || adv_state == A_STARTING || adv_state == A_RUNNING)
        adv_state = A_RUNNING;
    adv_dirty = true;
    link_t *l = NULL;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (!links[i].used)
        {
            l = &links[i];
            break;
        }
    if (!l || link_count() >= SLOT_COUNT || !hid_router_connect(&router, e->connection))
    {
        esp_ble_gap_disconnect((uint8_t *)e->address);
        return;
    }
    bool pending = unknown_pending();
    *l = (link_t){.used = true,
                  .connection = e->connection,
                  .mtu = 23,
                  .protocol = 1,
                  .candidate = -1,
                  .connected_at = esp_timer_get_time()};
    memcpy(l->address, e->address, 6);
    int slot = stored_slot(e->address);
    if (slot < 0 && pairing_open() && !pending)
        l->candidate = pairing_slot;
    ESP_LOGI(TAG, "Link connected conn=%u known_slot=%d links=%u/%u", l->connection, slot + 1, link_count(),
             SLOT_COUNT);
    if (slot == forgetting_slot && slot >= 0)
    {
        close_link(l);
        return;
    }
    // Unknown RPAs may resolve to an existing bonded identity at authentication.
    // Authorization below is mandatory before any notification is enabled.
    // Bonded centrals initiate encryption with their cached key. Do not start
    // competing SMP procedures on every link at once. Only new admissions need
    // an explicit security request, serialized below.
}

static void authenticated_event(const event_t *e)
{
    link_t *l = by_address(e->address);
    if (!l || l->closing)
        return;
    if (security_owner == (int)l->connection)
        security_owner = -1;
    l->security_request_pending = false;
    if (!e->flag)
    {
        if (e->status == ESP_AUTH_SMP_NUM_COMP_FAIL)
            ESP_LOGW(TAG,
                     "Authentication failed: 0x%x; check host pairing "
                     "agent/authorization (not necessarily a numeric-code prompt)",
                     e->status);
        else
            ESP_LOGW(TAG, "Authentication failed: 0x%x conn=%u", e->status, l->connection);
        close_link(l);
        return;
    }
    int slot = stored_slot(e->address);
    if (slot < 0)
    {
        if (!pairing_open() || l->candidate != pairing_slot)
        {
            ESP_LOGW(TAG, "Reject unassigned host outside its pairing window");
            close_link(l);
            return;
        }
        slot = pairing_slot;
    }
    if (slot == forgetting_slot || !hid_router_authorize(&router, l->connection, slot))
    {
        ESP_LOGW(TAG, "Duplicate or unavailable slot %d", slot + 1);
        close_link(l);
        return;
    }
    if (!store.slots[slot].used)
    {
        store.slots[slot].used = 1;
        store.slots[slot].type = e->address_type;
        memcpy(store.slots[slot].address, e->address, 6);
        save_slots();
        ESP_LOGI(TAG, "Paired host saved in slot %d", slot + 1);
        pairing_slot = -1;
        adv_dirty = true;
    }
    l->candidate = -1;
    l->cccd = saved_cccd[slot];
    subscriptions(l);
    ESP_LOGI(TAG, "Slot %d encrypted conn=%u; subscriptions=0x%02x", slot + 1, l->connection, l->cccd);
    // Attribute layout is compatible with the previous firmware. Bluedroid's
    // configured AUTO Service Changed policy handles any actual DB changes.
    esp_ble_conn_update_params_t params = {.min_int = 12, .max_int = 24, .latency = 0, .timeout = 400};
    memcpy(params.bda, l->address, 6);
    esp_ble_gap_update_conn_params(&params);
    // Re-encrypting a known host does not change admission policy. In
    // particular it must not race a simultaneous host's advertising restart.
}

static void finish_forget(void)
{
    int slot = forgetting_slot;
    memset(&store.slots[slot], 0, sizeof(slot_t));
    saved_cccd[slot] = 0;
    forgetting_slot = -1;
    removing_bond = false;
    save_slots();
    save_subscriptions();
    open_pairing(slot);
    adv_dirty = true;
    ESP_LOGI(TAG, "Slot %d forgotten; other links unchanged", slot + 1);
}

static void forget_tick(void)
{
    if (forgetting_slot < 0 || removing_bond)
        return;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        hid_peer_t *p = &router.peers[i];
        if (p->connected && p->slot == forgetting_slot)
        {
            close_link(by_id(p->connection));
            return;
        }
    }
    if (!store.slots[forgetting_slot].used)
    {
        finish_forget();
        return;
    }
    int count = esp_ble_get_bond_device_num();
    esp_ble_bond_dev_t bonds[CONFIG_BT_SMP_MAX_BONDS];
    if (count > CONFIG_BT_SMP_MAX_BONDS)
        count = CONFIG_BT_SMP_MAX_BONDS;
    bool present = false;
    if (count > 0 && esp_ble_get_bond_device_list(&count, bonds) == ESP_OK)
        for (int i = 0; i < count; ++i)
            if (!memcmp(bonds[i].bd_addr, store.slots[forgetting_slot].address, 6))
                present = true;
    if (!present)
    {
        finish_forget();
        return;
    }
    esp_err_t err = esp_ble_remove_bond_device(store.slots[forgetting_slot].address);
    if (err == ESP_OK)
        removing_bond = true;
    else
    {
        ESP_LOGE(TAG, "Cannot remove slot bond: %s", esp_err_to_name(err));
        forgetting_slot = -1;
    }
}

static void security_tick(void)
{
    int64_t now = esp_timer_get_time();
    if (security_owner >= 0)
    {
        link_t *l = by_id(security_owner);
        hid_peer_t *p = l ? hid_router_peer(&router, l->connection) : NULL;
        if (l && p && !p->encrypted && !l->closing && now - l->encryption_requested_at < 2000000)
        {
            if (l->security_request_pending)
            {
                esp_ble_gap_security_rsp(l->address, true);
                l->security_request_pending = false;
            }
            return;
        }
        security_owner = -1;
    }
    // Oldest outstanding request first, so a noisy peer cannot starve others.
    link_t *next = NULL;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        link_t *l = &links[i];
        hid_peer_t *p = l->used ? hid_router_peer(&router, l->connection) : NULL;
        if (!p || p->encrypted || l->closing || now - l->encryption_requested_at < 1000000)
            continue;
        if (stored_slot(l->address) >= 0 && !l->security_request_pending)
            continue;
        if (l->encryption_requested_at && !l->security_request_pending)
            continue;
        if (!next || l->encryption_requested_at < next->encryption_requested_at)
            next = l;
    }
    if (!next)
        return;
    security_owner = next->connection;
    next->encryption_requested_at = now;
    if (next->security_request_pending)
    {
        esp_ble_gap_security_rsp(next->address, true);
        next->security_request_pending = false;
    }
    else
    {
        esp_err_t err = esp_ble_set_encryption(next->address, ESP_BLE_SEC_ENCRYPT_NO_MITM);
        if (err != ESP_OK)
        {
            ESP_LOGW(TAG, "Encryption request conn=%u: %s", next->connection, esp_err_to_name(err));
            security_owner = -1;
        }
    }
}

static void handle_event(const event_t *e)
{
    switch (e->kind)
    {
    case E_REGISTER:
        ESP_ERROR_CHECK(e->status == ESP_GATT_OK ? ESP_OK : ESP_FAIL);
        gatt_if = e->handle;
        build_tables();
        ESP_ERROR_CHECK(esp_ble_gatts_create_attr_tab(battery_db, gatt_if, B_COUNT, 0));
        break;
    case E_TABLE:
        ESP_ERROR_CHECK(e->status == ESP_GATT_OK ? ESP_OK : ESP_FAIL);
        if (e->instance == 0 && e->count == B_COUNT)
        {
            memcpy(battery_handles, e->handles, sizeof(battery_handles));
            battery_include.start_hdl = battery_handles[0];
            battery_include.end_hdl = battery_handles[B_COUNT - 1];
            battery_include.uuid = u_battery;
            ESP_ERROR_CHECK(esp_ble_gatts_start_service(battery_handles[0]));
            ESP_ERROR_CHECK(esp_ble_gatts_create_attr_tab(info_db, gatt_if, I_COUNT, 1));
        }
        else if (e->instance == 1 && e->count == I_COUNT)
        {
            memcpy(info_handles, e->handles, sizeof(info_handles));
            ESP_ERROR_CHECK(esp_ble_gatts_start_service(info_handles[0]));
            ESP_ERROR_CHECK(esp_ble_gatts_create_attr_tab(hid_db, gatt_if, H_COUNT, 2));
        }
        else if (e->instance == 2 && e->count == H_COUNT)
        {
            memcpy(hid_handles, e->handles, sizeof(hid_handles));
            ESP_ERROR_CHECK(esp_ble_gatts_start_service(hid_handles[0]));
            ESP_LOGI(TAG, "HID handles: service=0x%04x key=0x%04x mouse=0x%04x consumer=0x%04x", hid_handles[H_SERVICE],
                     hid_handles[H_KEY], hid_handles[H_MOUSE], hid_handles[H_CONSUMER]);
        }
        else
            ESP_ERROR_CHECK(ESP_ERR_INVALID_SIZE);
        break;
    case E_SERVICE_STARTED:
        ESP_ERROR_CHECK(e->status == ESP_GATT_OK ? ESP_OK : ESP_FAIL);
        services_ready = ++started_services == 3;
        break;
    case E_CONNECT:
        connected_event(e);
        break;
    case E_DISCONNECT: {
        link_t *l = by_id(e->connection);
        if (!l || memcmp(l->address, e->address, 6))
            break;
        hid_peer_t *p = hid_router_peer(&router, e->connection);
        ESP_LOGI(TAG, "Disconnected conn=%u slot=%d reason=0x%x", e->connection, p ? p->slot + 1 : 0, e->status);
        if (security_owner == (int)e->connection)
            security_owner = -1;
        hid_router_disconnect(&router, e->connection);
        memset(l, 0, sizeof(*l));
        adv_dirty = true;
        break;
    }
    // Bluedroid also delivers READ callbacks for AUTO_RSP attributes, after it
    // has already answered them. Sending a second response corrupts ATT state.
    case E_READ:
        if (dynamic_handle(e->handle))
            read_value(e);
        break;
    case E_WRITE:
        write_value(e);
        break;
    case E_MTU: {
        link_t *l = by_id(e->connection);
        if (l)
            l->mtu = e->mtu;
        break;
    }
    case E_CONGESTION: {
        hid_peer_t *p = hid_router_peer(&router, e->connection);
        if (p)
            p->congested = e->flag;
        break;
    }
    case E_NOTIFY_ERROR: {
        hid_peer_t *p = hid_router_peer(&router, e->connection);
        if (p)
            p->releases |= p->subscriptions;
        ESP_LOGW(TAG, "Notification failed conn=%u status=%d; queue neutral reports", e->connection, e->status);
        break;
    }
    case E_AUTH:
        authenticated_event(e);
        break;
    case E_SECURITY: {
        link_t *l = by_address(e->address);
        bool allow =
            l && !l->closing && (stored_slot(e->address) >= 0 || (pairing_open() && l->candidate == pairing_slot));
        if (allow)
            l->security_request_pending = true;
        else
            esp_ble_gap_security_rsp((uint8_t *)e->address, false);
        break;
    }
    case E_NUMERIC:
        // NoInputNoOutput must not silently claim MITM numeric verification.
        ESP_LOGW(TAG, "Unexpected numeric comparison with NoInputNoOutput; rejecting");
        esp_ble_confirm_reply((uint8_t *)e->address, false);
        break;
    case E_ADV_DATA:
        data_ready = e->status == ESP_BT_STATUS_SUCCESS;
        break;
    case E_ADV_START:
        if (e->status == ESP_BT_STATUS_SUCCESS)
        {
            adv_state = A_RUNNING;
            ESP_LOGI(TAG, "Advertising reconnect=ALL_PAIRED pairing_slot=%d links=%u/%u",
                     pairing_open() ? pairing_slot + 1 : 0, link_count(), SLOT_COUNT);
        }
        else
            advertising_failed("start completion", e->status);
        break;
    case E_ADV_STOP:
        if (e->status == ESP_BT_STATUS_SUCCESS)
            adv_state = A_IDLE;
        else
            advertising_failed("stop completion", e->status);
        break;
    case E_WHITELIST:
        if (e->status != ESP_BT_STATUS_SUCCESS)
            advertising_failed("whitelist completion", e->status);
        else if (adv_state == A_CLEARING || adv_state == A_ADDING)
            whitelist_add_next();
        break;
    case E_BOND_REMOVED:
        if (forgetting_slot >= 0 && removing_bond && !memcmp(e->address, store.slots[forgetting_slot].address, 6))
        {
            if (e->status == ESP_BT_STATUS_SUCCESS)
                finish_forget();
            else
            {
                ESP_LOGE(TAG, "Bond removal failed: %d", e->status);
                forgetting_slot = -1;
                removing_bond = false;
            }
        }
        break;
    }
}

esp_err_t ble_switch_init(void)
{
    events = xQueueCreate(64, sizeof(event_t));
    if (!events)
        return ESP_ERR_NO_MEM;
    ESP_ERROR_CHECK(nvs_open("bt_switch", NVS_READWRITE, &nvs));
    size_t size = sizeof(store);
    esp_err_t err = nvs_get_blob(nvs, "slots", &store, &size);
    if (err != ESP_OK || size != sizeof(store) || store.version != 1 || store.selected >= SLOT_COUNT)
    {
        if (err != ESP_ERR_NVS_NOT_FOUND)
            ESP_LOGW(TAG, "Slot layout changed; old Bluetooth keys retained, no "
                          "automatic erase");
        memset(&store, 0, sizeof(store));
    }
    size = sizeof(saved_cccd);
    if (nvs_get_blob(nvs, "cccd", saved_cccd, &size) != ESP_OK || size != sizeof(saved_cccd))
        memset(saved_cccd, 0, sizeof(saved_cccd));
    hid_router_init(&router, SLOT_COUNT, store.selected, notify, NULL);
    esp_bt_controller_config_t cfg = BT_CONTROLLER_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_bt_controller_init(&cfg));
    ESP_ERROR_CHECK(esp_bt_controller_enable(ESP_BT_MODE_BLE));
    ESP_ERROR_CHECK(esp_bluedroid_init());
    ESP_ERROR_CHECK(esp_bluedroid_enable());
    ESP_ERROR_CHECK(esp_ble_gap_register_callback(gap_callback));
    ESP_ERROR_CHECK(esp_ble_gatts_register_callback(gatts_callback));
    uint8_t auth = ESP_LE_AUTH_REQ_SC_BOND, io = ESP_IO_CAP_NONE, key_size = 16;
    uint8_t keys = ESP_BLE_ENC_KEY_MASK | ESP_BLE_ID_KEY_MASK;
    ESP_ERROR_CHECK(esp_ble_gap_set_security_param(ESP_BLE_SM_AUTHEN_REQ_MODE, &auth, 1));
    ESP_ERROR_CHECK(esp_ble_gap_set_security_param(ESP_BLE_SM_IOCAP_MODE, &io, 1));
    ESP_ERROR_CHECK(esp_ble_gap_set_security_param(ESP_BLE_SM_MAX_KEY_SIZE, &key_size, 1));
    ESP_ERROR_CHECK(esp_ble_gap_set_security_param(ESP_BLE_SM_SET_INIT_KEY, &keys, 1));
    ESP_ERROR_CHECK(esp_ble_gap_set_security_param(ESP_BLE_SM_SET_RSP_KEY, &keys, 1));
    ESP_ERROR_CHECK(esp_ble_gap_set_device_name("BT Switch"));
    ESP_ERROR_CHECK(esp_ble_gap_config_local_icon(0x03c0));
    static uint8_t uuid[] = {0xfb, 0x34, 0x9b, 0x5f, 0x80, 0, 0, 0x80, 0, 0x10, 0, 0, 0x12, 0x18, 0, 0};
    esp_ble_adv_data_t data = {.include_name = true,
                               .appearance = 0x03c0,
                               .service_uuid_len = sizeof(uuid),
                               .p_service_uuid = uuid,
                               .flag = ESP_BLE_ADV_FLAG_GEN_DISC | ESP_BLE_ADV_FLAG_BREDR_NOT_SPT};
    ESP_ERROR_CHECK(esp_ble_gap_config_adv_data(&data));
    ESP_ERROR_CHECK(esp_ble_gatts_app_register(0x42));
    if (!store.slots[router.selected].used)
        open_pairing(router.selected);
    ESP_LOGI(TAG,
             "Multi-link HID initialized: capacity=%u; switching never "
             "disconnects peers",
             SLOT_COUNT);
    return ESP_OK;
}

void ble_switch_tick(void)
{
    event_t e;
    while (xQueueReceive(events, &e, 0) == pdTRUE)
        handle_event(&e);
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        link_t *l = &links[i];
        if (!l->used)
            continue;
        hid_peer_t *p = hid_router_peer(&router, l->connection);
        if (p && !p->encrypted && !l->closing && esp_timer_get_time() - l->connected_at > 30000000)
            close_link(l);
    }
    forget_tick();
    security_tick();
    advertising_tick();
    hid_router_flush(&router);
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        link_t *l = &links[i];
        hid_peer_t *p = l->used ? hid_router_peer(&router, l->connection) : NULL;
        bool available = p && p->encrypted && p->subscriptions && !p->releases && !p->suspended;
        if (available && !l->announced)
            ESP_LOGI(TAG, "Slot %d ready conn=%u (simultaneous links=%u)", p->slot + 1, l->connection, link_count());
        l->announced = available;
    }
}

void ble_switch_select(unsigned slot)
{
    if (slot >= SLOT_COUNT || slot == router.selected || forgetting_slot >= 0)
        return;
    hid_router_select(&router, slot);
    save_slots();
    if (!store.slots[slot].used)
        open_pairing(slot);
    else if (pairing_slot >= 0)
    {
        pairing_slot = -1;
        adv_dirty = true;
    }
    ESP_LOGI(TAG, "Input target=slot %u; %u links retained (no disconnect)", slot + 1, link_count());
}

void ble_switch_pair(void)
{
    if (store.slots[router.selected].used)
    {
        ESP_LOGW(TAG, "Slot occupied; choose an empty slot or explicitly forget it");
        return;
    }
    open_pairing(router.selected);
}

void ble_switch_forget(void)
{
    if (forgetting_slot >= 0)
        return;
    hid_router_release(&router);
    forgetting_slot = router.selected;
    pairing_slot = -1;
    adv_dirty = true;
    ESP_LOGW(TAG, "Forgetting only slot %u; remove BT Switch on that host too", router.selected + 1);
}

bool ble_switch_ready(void)
{
    return forgetting_slot != (int)router.selected && hid_router_ready(&router);
}
unsigned ble_switch_slot(void)
{
    return router.selected;
}
bool ble_switch_send(uint8_t report, uint8_t *data, unsigned size)
{
    return ble_switch_ready() && hid_router_send(&router, report, data, size);
}
void ble_switch_release(void)
{
    hid_router_release(&router);
}

bool ble_switch_probe(void)
{
    uint8_t zero[8] = {0};
    bool success = true;
    for (uint8_t report = 1; report <= 3; ++report)
        if (!ble_switch_send(report, zero, hid_router_report_size(report)))
            success = false;
    ESP_LOGI(TAG, "Neutral probe target=slot %u result=%s", router.selected + 1,
             success ? "sent" : "not ready/subscribed");
    return success;
}

void ble_switch_status(void)
{
    bool target_connected = false;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (router.peers[i].connected && router.peers[i].slot == (int)router.selected)
            target_connected = true;
    ESP_LOGI(TAG,
             "Selected slot=%u connected=%d ready=%d pairing=%d bonds=%d "
             "links=%u/%u mode=simultaneous",
             router.selected + 1, target_connected, ble_switch_ready(), pairing_open(), esp_ble_get_bond_device_num(),
             link_count(), SLOT_COUNT);
    for (unsigned s = 0; s < SLOT_COUNT; ++s)
    {
        hid_peer_t *p = NULL;
        for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
            if (router.peers[i].connected && router.peers[i].slot == (int)s)
                p = &router.peers[i];
        link_t *l = p ? by_id(p->connection) : NULL;
        ESP_LOGI(TAG,
                 "  Slot %u: %s connected=%d encrypted=%d notify=0x%02x conn=%d "
                 "tx=%" PRIu32 ",%" PRIu32 ",%" PRIu32 "%s",
                 s + 1, store.slots[s].used ? "paired" : "empty", p != NULL, p && p->encrypted,
                 p ? p->subscriptions : 0, p ? (int)p->connection : -1, l ? l->sent[0] : 0, l ? l->sent[1] : 0,
                 l ? l->sent[2] : 0, s == router.selected ? " INPUT_TARGET" : "");
    }
}

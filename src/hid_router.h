#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Internal portable seam. Transport and tests supply the same notification
// sink.
#define HID_ROUTER_MAX_PEERS 6
#define HID_ROUTER_REPORTS 3
#define HID_ROUTER_UNBOUND (-1)
typedef bool (*hid_notify_fn)(void *context, uint16_t connection, uint8_t report, const uint8_t *data, size_t length);
typedef struct
{
    bool connected, encrypted, suspended, congested;
    uint16_t connection;
    int slot;
    uint8_t subscriptions, releases;
    uint8_t values[HID_ROUTER_REPORTS][8];
} hid_peer_t;
typedef struct
{
    unsigned slot_count, selected;
    hid_peer_t peers[HID_ROUTER_MAX_PEERS];
    hid_notify_fn notify;
    void *context;
} hid_router_t;

void hid_router_init(hid_router_t *router, unsigned slot_count, unsigned selected, hid_notify_fn notify, void *context);
hid_peer_t *hid_router_peer(hid_router_t *router, uint16_t connection);
bool hid_router_connect(hid_router_t *router, uint16_t connection);
void hid_router_disconnect(hid_router_t *router, uint16_t connection);
bool hid_router_authorize(hid_router_t *router, uint16_t connection, unsigned slot);
void hid_router_subscribe(hid_router_t *router, uint16_t connection, uint8_t mask);
void hid_router_select(hid_router_t *router, unsigned slot);
void hid_router_release(hid_router_t *router);
void hid_router_flush(hid_router_t *router);
bool hid_router_ready(const hid_router_t *router);
bool hid_router_send(hid_router_t *router, uint8_t report, const uint8_t *data, size_t length);
size_t hid_router_report_size(uint8_t report);

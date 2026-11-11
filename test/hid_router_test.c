#include "hid_router.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct
{
    uint16_t connection;
    uint8_t report, data[8];
    size_t size;
} packet_t;
static packet_t packets[128];
static unsigned count;
static uint16_t blocked = UINT16_MAX;
static bool notify(void *context, uint16_t connection, uint8_t report, const uint8_t *data, size_t size)
{
    (void)context;
    if (connection == blocked)
        return false;
    assert(count < 128);
    packets[count] = (packet_t){.connection = connection, .report = report, .size = size};
    memcpy(packets[count++].data, data, size);
    return true;
}
static void online(hid_router_t *r, unsigned id, unsigned slot)
{
    assert(hid_router_connect(r, id));
    assert(hid_router_authorize(r, id, slot));
    hid_router_subscribe(r, id, 7);
    hid_router_flush(r);
}
static void zero_packets(unsigned begin)
{
    for (unsigned i = begin; i < count; ++i)
        for (unsigned j = 0; j < packets[i].size; ++j)
            assert(packets[i].data[j] == 0);
}
int main(void)
{
    hid_router_t r;
    hid_router_init(&r, 3, 0, notify, NULL);
    online(&r, 10, 0);
    online(&r, 20, 1);
    online(&r, 30, 2);
    assert(hid_router_ready(&r));
    count = 0;
    uint8_t key[8] = {1, 0, 4}, mouse[7] = {1, 10, 0, 0, 0, 0, 0}, consumer[2] = {0xcd, 0};
    assert(hid_router_send(&r, 1, key, 8));
    assert(hid_router_send(&r, 2, mouse, 7));
    assert(hid_router_send(&r, 3, consumer, 2));
    for (unsigned i = 0; i < count; ++i)
        assert(packets[i].connection == 10);
    assert(hid_router_authorize(&r, 10, 0));
    assert(hid_router_peer(&r, 10)->releases == 0); // duplicate auth must not release a held key
    unsigned before = count;
    hid_router_select(&r, 1);
    assert(count == before + 6);
    zero_packets(before);
    for (unsigned i = 0; i < 3; ++i)
        assert(r.peers[i].connected && r.peers[i].encrypted);
    assert(hid_router_send(&r, 1, key, 8));
    assert(packets[count - 1].connection == 20);
    puts("PASS simultaneous links, keyboard/mouse/consumer target routing, release old and new host");

    blocked = 20;
    before = count;
    hid_router_select(&r, 2);
    assert(hid_router_peer(&r, 20)->releases == 7);
    assert(hid_router_ready(&r)); // old host congestion must not block new host
    assert(hid_router_send(&r, 1, key, 8));
    assert(packets[count - 1].connection == 30);
    blocked = UINT16_MAX;
    before = count;
    hid_router_flush(&r);
    assert(count == before + 3);
    zero_packets(before);
    puts("PASS congestion defers old-host releases without disconnecting or leaking input");

    hid_router_disconnect(&r, 30);
    assert(!hid_router_ready(&r));
    assert(!hid_router_send(&r, 1, key, 8));
    assert(hid_router_peer(&r, 10) && hid_router_peer(&r, 20));
    online(&r, 31, 2);
    assert(hid_router_ready(&r));
    assert(!hid_router_connect(&r, 31));
    assert(hid_router_connect(&r, 99));
    assert(!hid_router_authorize(&r, 99, 2));
    hid_router_disconnect(&r, 99);
    assert(hid_router_peer(&r, 31));
    hid_router_disconnect(&r, 99); // stale duplicate disconnect cannot affect other connections
    puts("PASS offline target, independent reconnect, duplicate and unbound connection isolation");

    hid_router_subscribe(&r, 31, 1);
    hid_router_flush(&r);
    assert(!hid_router_send(&r, 2, mouse, 7));
    assert(hid_router_send(&r, 1, key, 8));
    hid_router_peer(&r, 31)->suspended = true;
    assert(!hid_router_ready(&r));
    hid_router_select(&r, 0);
    assert(hid_router_ready(&r));
    assert(hid_router_peer(&r, 31)->suspended);
    assert(!hid_router_send(&r, 0, key, 8));
    assert(!hid_router_send(&r, 1, key, 7));
    hid_router_peer(&r, 10)->encrypted = false;
    assert(!hid_router_send(&r, 1, key, 8));
    puts("PASS per-peer CCCD, suspend, encryption gates, malformed input rejection");

    hid_router_init(&r, 6, 0, notify, NULL);
    count = 0;
    for (unsigned i = 0; i < 6; ++i)
        online(&r, 100 + i, i);
    assert(!hid_router_connect(&r, 200));
    for (unsigned i = 0; i < 6; ++i)
    {
        hid_router_select(&r, i);
        assert(hid_router_send(&r, 1, key, 8));
        assert(packets[count - 1].connection == 100 + i);
        for (unsigned j = 0; j < 6; ++j)
            assert(hid_router_peer(&r, 100 + j));
    }
    puts("PASS six-slot capacity and all targets retain independent live links");
    return 0;
}

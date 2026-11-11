#include "hid_router.h"
#include <string.h>

size_t hid_router_report_size(uint8_t report)
{
    static const uint8_t sizes[] = {8, 7, 2};
    return report >= 1 && report <= HID_ROUTER_REPORTS ? sizes[report - 1] : 0;
}

void hid_router_init(hid_router_t *r, unsigned slots, unsigned selected, hid_notify_fn notify, void *context)
{
    memset(r, 0, sizeof(*r));
    r->slot_count = slots <= HID_ROUTER_MAX_PEERS ? slots : HID_ROUTER_MAX_PEERS;
    r->selected = selected < r->slot_count ? selected : 0;
    r->notify = notify;
    r->context = context;
}

hid_peer_t *hid_router_peer(hid_router_t *r, uint16_t connection)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
        if (r->peers[i].connected && r->peers[i].connection == connection)
            return &r->peers[i];
    return NULL;
}

bool hid_router_connect(hid_router_t *r, uint16_t connection)
{
    if (hid_router_peer(r, connection))
        return false; // duplicate stack event
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        if (r->peers[i].connected)
            continue;
        r->peers[i] = (hid_peer_t){.connected = true, .connection = connection, .slot = HID_ROUTER_UNBOUND};
        return true;
    }
    return false;
}

void hid_router_disconnect(hid_router_t *r, uint16_t connection)
{
    hid_peer_t *p = hid_router_peer(r, connection);
    if (p)
        memset(p, 0, sizeof(*p));
}

bool hid_router_authorize(hid_router_t *r, uint16_t connection, unsigned slot)
{
    hid_peer_t *p = hid_router_peer(r, connection);
    if (!p || slot >= r->slot_count)
        return false;
    if (p->encrypted && p->slot == (int)slot)
        return true;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        hid_peer_t *other = &r->peers[i];
        if (other != p && other->connected && other->slot == (int)slot)
            return false;
    }
    p->slot = slot;
    p->encrypted = true;
    p->releases |= p->subscriptions;
    return true;
}

void hid_router_subscribe(hid_router_t *r, uint16_t connection, uint8_t mask)
{
    hid_peer_t *p = hid_router_peer(r, connection);
    if (!p)
        return;
    mask &= 7;
    p->releases |= mask & ~p->subscriptions;
    p->subscriptions = mask;
    p->releases &= mask;
}

static bool can_notify(const hid_peer_t *p)
{
    return p->connected && p->encrypted && p->slot >= 0 && !p->suspended && !p->congested;
}

void hid_router_flush(hid_router_t *r)
{
    const uint8_t zero[8] = {0};
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        hid_peer_t *p = &r->peers[i];
        if (!can_notify(p))
            continue;
        for (uint8_t report = 1; report <= HID_ROUTER_REPORTS; ++report)
        {
            unsigned bit = 1u << (report - 1);
            if (!(p->releases & p->subscriptions & bit))
                continue;
            if (r->notify(r->context, p->connection, report, zero, hid_router_report_size(report)))
            {
                p->releases &= ~bit;
                memset(p->values[report - 1], 0, sizeof(p->values[report - 1]));
            }
        }
    }
}

void hid_router_release(hid_router_t *r)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        hid_peer_t *p = &r->peers[i];
        if (p->connected && p->slot == (int)r->selected)
            p->releases |= p->subscriptions;
    }
    hid_router_flush(r);
}

void hid_router_select(hid_router_t *r, unsigned slot)
{
    if (slot >= r->slot_count || slot == r->selected)
        return;
    // Releasing a congested old host is deferred, never forgotten. No transport
    // disconnect is part of this operation, even when the new slot is offline.
    hid_router_release(r);
    r->selected = slot;
    hid_router_release(r);
}

bool hid_router_ready(const hid_router_t *r)
{
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        const hid_peer_t *p = &r->peers[i];
        if (p->slot == (int)r->selected && can_notify(p) && p->subscriptions && !p->releases)
            return true;
    }
    return false;
}

bool hid_router_send(hid_router_t *r, uint8_t report, const uint8_t *data, size_t length)
{
    if (!length || length != hid_router_report_size(report) || !data)
        return false;
    for (unsigned i = 0; i < HID_ROUTER_MAX_PEERS; ++i)
    {
        hid_peer_t *p = &r->peers[i];
        if (p->slot != (int)r->selected || !can_notify(p) || p->releases || !(p->subscriptions & (1u << (report - 1))))
            continue;
        if (!r->notify(r->context, p->connection, report, data, length))
            return false;
        memcpy(p->values[report - 1], data, length);
        if (report == 2)
            memset(p->values[1] + 1, 0, 6); // relative motion is never replayed
        return true;
    }
    return false;
}

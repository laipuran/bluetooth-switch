#include "hid_parser.h"
#include <limits.h>
#include <string.h>

typedef struct
{
    uint32_t page, size, count, id;
    int32_t minimum;
} globals_t;

static int32_t signed_value(uint32_t value, unsigned bits)
{
    if (!bits)
        return 0;
    if (bits < 32 && (value & (1u << (bits - 1))))
        value |= UINT32_MAX << bits;
    return (int32_t)value;
}

static uint8_t kind(uint16_t page, uint16_t usage, bool variable)
{
    if (page == 7)
        return HID_KEYBOARD;
    if (page == 9 && variable && usage >= 1 && usage <= 8)
        return HID_MOUSE;
    if (page == 1 && variable && (usage == 0x30 || usage == 0x31 || usage == 0x38))
        return HID_MOUSE;
    if (page == 12 && usage == 0x238 && variable)
        return HID_MOUSE;
    if (page == 12)
        return HID_CONSUMER;
    return 0;
}

bool hid_parser_init(hid_parser_t *p, const uint8_t *desc, size_t length)
{
    memset(p, 0, sizeof(*p));
    globals_t g = {0}, stack[8];
    unsigned depth = 0;
    bool mouse_allowed = true, collection_stack[16];
    unsigned collection_depth = 0;
    uint32_t usages[64], minimum = 0, maximum = 0;
    unsigned usage_count = 0;
    bool range = false;
    for (size_t i = 0; i < length;)
    {
        uint8_t prefix = desc[i++];
        if (prefix == 0xfe)
        { // Long items are not interpreted.
            if (i + 2 > length)
                return false;
            unsigned n = desc[i];
            i += 2;
            if (i + n > length)
                return false;
            i += n;
            continue;
        }
        unsigned n = prefix & 3;
        if (n == 3)
            n = 4;
        if (i + n > length)
            return false;
        uint32_t v = 0;
        for (unsigned b = 0; b < n; ++b)
            v |= (uint32_t)desc[i++] << (b * 8);
        unsigned type = (prefix >> 2) & 3, tag = prefix >> 4;
        if (type == 1)
        {
            switch (tag)
            {
            case 0:
                g.page = v;
                break;
            case 1:
                g.minimum = signed_value(v, n * 8);
                break;
            case 7:
                g.size = v;
                break;
            case 8:
                if (!v || v > 255)
                    return false;
                g.id = v;
                p->numbered = true;
                break;
            case 9:
                g.count = v;
                break;
            case 10:
                if (depth == 8)
                    return false;
                stack[depth++] = g;
                break;
            case 11:
                if (!depth)
                    return false;
                g = stack[--depth];
                break;
            default:
                break;
            }
        }
        else if (type == 2)
        {
            uint32_t u = n == 4 ? v : (g.page << 16) | v;
            if (tag == 0)
            {
                if (usage_count == 64)
                    return false;
                usages[usage_count++] = u;
            }
            else if (tag == 1)
            {
                minimum = u;
                range = true;
            }
            else if (tag == 2)
                maximum = u;
            else if (tag == 10)
                return false; // Delimiter alternatives unsupported.
        }
        else if (type == 0)
        {
            if (tag == 10)
            {
                if (collection_depth == 16)
                    return false;
                collection_stack[collection_depth++] = mouse_allowed;
                if (v == 1)
                {
                    uint32_t u = usage_count ? usages[0] : minimum;
                    mouse_allowed = u == 0x00010002;
                }
            }
            else if (tag == 12)
            {
                if (!collection_depth)
                    return false;
                mouse_allowed = collection_stack[--collection_depth];
            }
            if (tag == 8)
            {
                if (!g.size || g.size > 32 || !g.count || g.count > 4096)
                    return false;
                unsigned r;
                for (r = 0; r < p->report_count; ++r)
                    if (p->reports[r].id == g.id)
                        break;
                if (r == p->report_count)
                {
                    if (r == HID_MAX_REPORTS)
                        return false;
                    p->reports[r].id = g.id;
                    ++p->report_count;
                }
                hid_layout_t *layout = &p->reports[r];
                uint64_t end = layout->bits + (uint64_t)g.size * g.count;
                if (end > 8192)
                    return false;
                if (!(v & 1))
                {
                    if (range && maximum < minimum)
                        return false;
                    // Non-contiguous array selector lists need a lookup table;
                    // reject them rather than silently translate the wrong key.
                    if (!(v & 2) && !range)
                    {
                        for (unsigned j = 1; j < usage_count; ++j)
                            if (usages[j] != usages[0] + j)
                                return false;
                    }
                    for (uint32_t f = 0; f < g.count; ++f)
                    {
                        uint32_t u = range ? minimum : (usage_count ? usages[0] : g.page << 16);
                        if (v & 2)
                        {
                            if (range)
                                u = minimum + (f <= maximum - minimum ? f : maximum - minimum);
                            else if (usage_count)
                                u = usages[f < usage_count ? f : usage_count - 1];
                        }
                        uint8_t k = kind(u >> 16, u & 0xffff, v & 2);
                        if (k == HID_MOUSE && !mouse_allowed)
                            k = 0;
                        // Absolute axes (tablets, gamepads) must not become mouse motion.
                        if (k == HID_MOUSE && (u >> 16) != 9 && !(v & 4))
                            k = 0;
                        if (!k)
                            continue;
                        if (p->field_count == HID_MAX_FIELDS)
                            return false;
                        p->fields[p->field_count++] = (hid_field_t){
                            .offset = layout->bits + f * g.size,
                            .size = g.size,
                            .usage = u & 0xffff,
                            .page = u >> 16,
                            .usage_max = range ? maximum & 0xffff
                                               : (usage_count ? usages[usage_count - 1] & 0xffff : UINT16_MAX),
                            .report = r,
                            .flags = v,
                            .logical_min = g.minimum,
                        };
                        layout->kinds |= k;
                    }
                }
                layout->bits = end;
            }
            usage_count = 0;
            minimum = maximum = 0;
            range = false;
        }
    }
    if (depth || collection_depth)
        return false;
    if (p->numbered)
    {
        for (unsigned i = 0; i < p->report_count; ++i)
            if (!p->reports[i].id)
                return false;
    }
    return p->field_count != 0;
}

bool hid_parser_decode(const hid_parser_t *p, const uint8_t *data, size_t size, hid_input_t *out, uint8_t *layout_index)
{
    memset(out, 0, sizeof(*out));
    if (!size)
        return false;
    uint8_t id = 0;
    if (p->numbered)
    {
        id = *data++;
        --size;
    }
    unsigned r;
    for (r = 0; r < p->report_count; ++r)
        if (p->reports[r].id == id)
            break;
    if (r == p->report_count || size < (p->reports[r].bits + 7u) / 8u)
        return false;
    *layout_index = r;
    out->kinds = p->reports[r].kinds;
    for (unsigned i = 0; i < p->field_count; ++i)
    {
        const hid_field_t *f = &p->fields[i];
        if (f->report != r)
            continue;
        uint32_t raw = 0;
        for (unsigned b = 0; b < f->size; ++b)
        {
            unsigned bit = f->offset + b;
            raw |= (uint32_t)((data[bit / 8] >> (bit % 8)) & 1) << b;
        }
        int32_t value = f->logical_min < 0 ? signed_value(raw, f->size) : (int32_t)raw;
        bool variable = f->flags & 2;
        uint32_t u = f->usage;
        if (!variable)
        {
            // A zero array element represents no key for supported HID usages.
            if (!raw)
                continue;
            int64_t mapped = (int64_t)f->usage + value - f->logical_min;
            if (mapped < 0 || mapped > f->usage_max)
                continue;
            u = mapped;
        }
        else if (!value)
            continue;
        if (f->page == 7 && u < 256)
            out->keys[u / 8] |= 1u << (u % 8);
        else if (f->page == 9 && u >= 1 && u <= 8)
            out->buttons |= 1u << (u - 1);
        else if (f->page == 1)
        {
            if (u == 0x30)
                out->x = value;
            if (u == 0x31)
                out->y = value;
            if (u == 0x38)
                out->wheel = value;
        }
        else if (f->page == 12)
        {
            if (u == 0x238 && variable)
                out->pan = value;
            else if (!out->consumer)
                out->consumer = u;
        }
    }
    return true;
}

#include "hid_parser.h"
#include "hid_profile.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static hid_parser_t parser;
static hid_input_t input;
static uint8_t index;

static void keyboard_test(void)
{
    const uint8_t desc[] = {
        0x05,
        1,
        0x09,
        6,
        0xa1,
        1,
        0x05,
        7,
        0x19,
        0xe0,
        0x29,
        0xe7,
        0x15,
        0,
        0x25,
        1,
        0x75,
        1,
        0x95,
        8,
        0x81,
        2,
        0x75,
        8,
        0x95,
        1,
        0x81,
        1,
        // Output reports must not advance Input bit offsets.
        0x05,
        8,
        0x19,
        1,
        0x29,
        5,
        0x75,
        1,
        0x95,
        5,
        0x91,
        2,
        0x75,
        3,
        0x95,
        1,
        0x91,
        1,
        0x05,
        7,
        0x19,
        0,
        0x29,
        0x65,
        0x15,
        0,
        0x25,
        0x65,
        0x75,
        8,
        0x95,
        6,
        0x81,
        0,
        0xc0,
    };
    assert(hid_parser_init(&parser, desc, sizeof(desc)));
    assert(parser.reports[0].bits == 64);
    const uint8_t report[] = {2, 0, 4, 5, 0, 0, 0, 0};
    assert(hid_parser_decode(&parser, report, sizeof(report), &input, &index));
    assert(input.kinds == HID_KEYBOARD);
    assert(input.keys[0] == ((1 << 4) | (1 << 5)));
    assert(input.keys[0xe1 / 8] & (1 << (0xe1 % 8)));
    assert(!hid_parser_decode(&parser, report, 7, &input, &index));
    uint8_t release[8] = {0};
    assert(hid_parser_decode(&parser, release, 8, &input, &index));
    for (unsigned i = 0; i < HID_KEYS_BYTES; ++i)
        assert(!input.keys[i]);
    puts("PASS keyboard modifiers, array keys, output offsets, release, truncation");
}

static void nkro_test(void)
{
    const uint8_t desc[] = {
        0x05, 7, 0x85, 9, 0x19, 0, 0x29, 0xe7, 0x15, 0, 0x25, 1, 0x75, 1, 0x96, 0xe8, 0, 0x81, 2,
    };
    assert(hid_parser_init(&parser, desc, sizeof(desc)));
    uint8_t report[30] = {9};
    report[1] = 0x10;
    report[29] = 0x80;
    assert(hid_parser_decode(&parser, report, sizeof(report), &input, &index));
    assert(input.keys[0] == 0x10 && input.keys[28] == 0x80);
    report[0] = 8;
    assert(!hid_parser_decode(&parser, report, sizeof(report), &input, &index));
    puts("PASS 232-bit NKRO and numbered/unknown report IDs");
}

static void mouse_test(void)
{
    const uint8_t desc[] = {
        0x05, 9,    0x19, 1,    0x29, 8, 0x15, 0,    0x25, 1, 0x75, 1,  0x95, 8,    0x81, 2,    0x05, 1,
        0x09, 0x30, 0x09, 0x31, 0x16, 0, 0xf8, 0x26, 0xff, 7, 0x75, 12, 0x95, 2,    0x81, 6,    0x09, 0x38,
        0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 1,    0x81, 6, 0x05, 12, 0x0a, 0x38, 2,    0x81, 6,
    };
    assert(hid_parser_init(&parser, desc, sizeof(desc)));
    // Packed X=-2 (FFE), Y=1024 (400).
    const uint8_t data[] = {0x81, 0xfe, 0x0f, 0x40, 0xff, 2};
    assert(hid_parser_decode(&parser, data, sizeof(data), &input, &index));
    assert(input.buttons == 0x81 && input.x == -2 && input.y == 1024);
    assert(input.wheel == -1 && input.pan == 2);
    puts("PASS packed 12-bit signed axes, 8 buttons, wheel and horizontal pan");
}

static void report_ids_test(void)
{
    const uint8_t desc[] = {
        0x05, 12, 0x85, 2,    0x19, 0,    0x2a, 0xff, 3,    0x15, 0,    0x26, 0xff, 3, 0x75, 16, 0x95, 1, 0x81, 0,
        0x85, 3,  0x09, 0xe9, 0x09, 0xea, 0x15, 0,    0x25, 1,    0x75, 1,    0x95, 2, 0x81, 2,  0x95, 6, 0x81, 1,
    };
    assert(hid_parser_init(&parser, desc, sizeof(desc)));
    const uint8_t data[] = {2, 0xcd, 0};
    assert(hid_parser_decode(&parser, data, sizeof(data), &input, &index));
    assert(index == 0 && input.consumer == 0xcd && input.kinds == HID_CONSUMER);
    const uint8_t volume[] = {3, 2};
    assert(hid_parser_decode(&parser, volume, sizeof(volume), &input, &index));
    assert(index == 1 && input.consumer == 0xea);
    puts("PASS consumer arrays/bitmaps and independent report layouts");
}

static void absolute_axes_test(void)
{
    const uint8_t desc[] = {0x05, 1, 0x09, 0x30, 0x15, 0, 0x26, 0xff, 0x7f, 0x75, 16, 0x95, 1, 0x81, 2};
    assert(!hid_parser_init(&parser, desc, sizeof(desc)));
    const uint8_t gamepad[] = {
        0x05, 1, 0x09, 5, 0xa1, 1, 0x05, 9, 0x19, 1, 0x29, 8, 0x15, 0, 0x25, 1, 0x75, 1, 0x95, 8, 0x81, 2, 0xc0,
    };
    assert(!hid_parser_init(&parser, gamepad, sizeof(gamepad)));
    puts("PASS absolute axes are not interpreted as relative mouse motion");
}

static void malformed_test(void)
{
    const uint8_t truncated[] = {0x75};
    assert(!hid_parser_init(&parser, truncated, sizeof(truncated)));
    const uint8_t overflow[] = {0x05, 7, 0x19, 0, 0x29, 0xff, 0x75, 32, 0x96, 0xff, 0xff, 0x81, 2};
    assert(!hid_parser_init(&parser, overflow, sizeof(overflow)));
    const uint8_t pop[] = {0xb4};
    assert(!hid_parser_init(&parser, pop, sizeof(pop)));
    const uint8_t id_zero[] = {0x85, 0};
    assert(!hid_parser_init(&parser, id_zero, sizeof(id_zero)));
    uint32_t seed = 0x12345678;
    for (unsigned run = 0; run < 20000; ++run)
    {
        uint8_t bytes[128];
        for (unsigned i = 0; i < sizeof(bytes); ++i)
        {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            bytes[i] = seed;
        }
        if (hid_parser_init(&parser, bytes, run % sizeof(bytes)))
        {
            hid_parser_decode(&parser, bytes, sizeof(bytes), &input, &index);
        }
    }
    puts("PASS malformed descriptors and 20,000 deterministic fuzz inputs");
}

int main(void)
{
    assert(hid_parser_init(&parser, report_map, sizeof(report_map)));
    assert(parser.report_count == 3 && parser.reports[0].bits == 64 && parser.reports[1].bits == 56 &&
           parser.reports[2].bits == 16);
    const uint8_t live_mouse[] = {2, 1, 0x2c, 1, 0xd4, 0xfe, 1, 0xff};
    assert(hid_parser_decode(&parser, live_mouse, sizeof(live_mouse), &input, &index));
    assert(input.buttons == 1 && input.x == 300 && input.y == -300 && input.wheel == 1 && input.pan == -1);
    assert(H_KEY == 12 && H_MOUSE == 24 && H_CONSUMER == 31 && H_COUNT == 34);
    puts("PASS production BLE profile, signed 16-bit motion and legacy GATT handle offsets");
    keyboard_test();
    nkro_test();
    mouse_test();
    report_ids_test();
    absolute_axes_test();
    malformed_test();
    return 0;
}

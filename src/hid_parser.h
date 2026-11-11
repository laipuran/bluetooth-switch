#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Bounded descriptor interpreter: no allocation, independent of ESP-IDF. */
#define HID_MAX_FIELDS 320
#define HID_MAX_REPORTS 16
#define HID_KEYS_BYTES 32

enum
{
    HID_KEYBOARD = 1,
    HID_MOUSE = 2,
    HID_CONSUMER = 4
};

typedef struct
{
    uint8_t kinds;
    uint8_t keys[HID_KEYS_BYTES];
    uint8_t buttons;
    int32_t x, y, wheel, pan;
    uint16_t consumer;
} hid_input_t;

typedef struct
{
    uint16_t offset, usage, usage_max, page;
    uint8_t size, report, flags;
    int32_t logical_min;
} hid_field_t;

typedef struct
{
    uint8_t id;
    uint16_t bits;
    uint8_t kinds;
} hid_layout_t;

typedef struct
{
    hid_field_t fields[HID_MAX_FIELDS];
    hid_layout_t reports[HID_MAX_REPORTS];
    uint16_t field_count;
    uint8_t report_count;
    bool numbered;
} hid_parser_t;

bool hid_parser_init(hid_parser_t *parser, const uint8_t *descriptor, size_t size);
bool hid_parser_decode(const hid_parser_t *parser, const uint8_t *data, size_t size, hid_input_t *out,
                       uint8_t *layout_index);

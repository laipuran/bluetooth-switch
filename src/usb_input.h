#pragma once
#include "esp_err.h"
#include "hid_parser.h"

#define USB_INPUT_SOURCES 8
typedef struct
{
    uint8_t source;
    hid_input_t input;
} usb_input_event_t;

esp_err_t usb_input_init(void);
bool usb_input_receive(usb_input_event_t *event);
bool usb_input_overflowed(void);
void usb_input_status(void);

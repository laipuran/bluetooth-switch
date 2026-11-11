#pragma once
#include "esp_err.h"
#include <stdbool.h>
#include <stdint.h>

// Application-task interface: call tick regularly; do not invoke these methods
// from an ISR or Bluetooth callback. Stack callbacks are queued internally.
// Selecting a slot changes only the input target, never other live links.
// ready means the target is encrypted, subscribed and its neutral reports have
// been queued. Failed/congested releases are retried by tick, including for an
// inactive old target. send returns false if this report cannot be queued.
esp_err_t ble_switch_init(void);
void ble_switch_tick(void);
void ble_switch_select(unsigned slot);
void ble_switch_pair(void);
void ble_switch_forget(void);
void ble_switch_status(void);
unsigned ble_switch_slot(void);
bool ble_switch_ready(void);
bool ble_switch_send(uint8_t report, uint8_t *data, unsigned size);
void ble_switch_release(void);
// Safe transport probe: neutral reports only, no key presses or pointer motion.
bool ble_switch_probe(void);

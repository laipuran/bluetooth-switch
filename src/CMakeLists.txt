idf_component_register(
    SRCS "main.c" "ble_switch.c" "hid_router.c" "usb_input.c" "hid_parser.c"
    INCLUDE_DIRS "."
    REQUIRES bt nvs_flash esp_driver_gpio esp_timer
)

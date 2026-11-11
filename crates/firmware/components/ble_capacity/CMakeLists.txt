# 多条主机可同时发起加密，即使应用主动请求串行，也不能只分配一个 SMP 状态机。
# ESP-IDF v6.1 默认 MYNEWT_VAL_BLE_SM_MAX_PROCS=1，且没有对应的 Kconfig 项。
idf_component_register(REQUIRES bt)

if(CONFIG_BT_NIMBLE_ENABLED)
    idf_component_get_property(bt_library bt COMPONENT_LIB)
    target_compile_definitions(${bt_library} PRIVATE
        MYNEWT_VAL_BLE_SM_MAX_PROCS=${CONFIG_BT_NIMBLE_MAX_CONNECTIONS})
endif()

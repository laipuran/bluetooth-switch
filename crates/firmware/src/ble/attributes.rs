//! 栈回调访问的逐连接属性快照；锁内只处理内存，不调用蓝牙栈。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bt_switch_core::session::HidSession;

#[derive(Clone, Default)]
pub(super) struct Sessions(Arc<Mutex<BTreeMap<u16, HidSession>>>);

impl Sessions {
    pub(super) fn connect(&self, connection: u16) {
        self.0
            .lock()
            .unwrap()
            .insert(connection, HidSession::default());
    }

    pub(super) fn disconnect(&self, connection: u16) {
        self.0.lock().unwrap().remove(&connection);
    }

    pub(super) fn get(&self, connection: u16) -> Option<HidSession> {
        self.0.lock().unwrap().get(&connection).cloned()
    }

    pub(super) fn update(&self, connection: u16, change: impl FnOnce(&mut HidSession)) -> bool {
        let mut sessions = self.0.lock().unwrap();
        let Some(session) = sessions.get_mut(&connection) else {
            return false;
        };
        change(session);
        true
    }
}

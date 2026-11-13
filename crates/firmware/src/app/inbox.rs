//! 有界事件批次：首条输入唤醒任务，随后只取有限数量，给管理逻辑保留运行机会。
//! 仅依赖标准库，可在主机上测试实际使用的接收逻辑。

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

pub(crate) const MAX_EVENTS_PER_TURN: usize = 16;

pub(crate) fn receive_batch<T>(
    receiver: &Receiver<T>,
    wait: Duration,
    mut consume: impl FnMut(T),
) -> Result<usize, RecvTimeoutError> {
    let first = match receiver.recv_timeout(wait) {
        Ok(event) => event,
        Err(RecvTimeoutError::Timeout) => return Ok(0),
        Err(error) => return Err(error),
    };
    consume(first);

    let mut count = 1;
    while count < MAX_EVENTS_PER_TURN {
        let Ok(event) = receiver.try_recv() else {
            break;
        };
        consume(event);
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn ready_events_are_consumed_in_order_without_waiting_for_a_full_batch() {
        let (tx, rx) = mpsc::channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        let mut seen = Vec::new();
        assert_eq!(
            receive_batch(&rx, Duration::ZERO, |event| seen.push(event)),
            Ok(2)
        );
        assert_eq!(seen, [1, 2]);
    }

    #[test]
    fn a_busy_producer_cannot_keep_the_consumer_in_one_turn_forever() {
        let (tx, rx) = mpsc::channel();
        tx.send(0).unwrap();
        let mut seen = Vec::new();
        let count = receive_batch(&rx, Duration::ZERO, |event| {
            seen.push(event);
            // 模拟每消费一条就产生新输入，队列始终不空。
            tx.send(event + 1).unwrap();
        })
        .unwrap();
        assert_eq!(count, MAX_EVENTS_PER_TURN);
        assert_eq!(seen, (0..MAX_EVENTS_PER_TURN).collect::<Vec<_>>());
        assert_eq!(rx.try_recv().unwrap(), MAX_EVENTS_PER_TURN);
    }

    #[test]
    fn idle_timeout_still_returns_control_for_management_and_release_retries() {
        let (_tx, rx) = mpsc::channel::<u8>();
        assert_eq!(
            receive_batch(&rx, Duration::ZERO, |_| panic!("no event")),
            Ok(0)
        );
    }

    #[test]
    fn disconnect_is_distinct_from_idle_and_queued_events_are_not_lost() {
        let (tx, rx) = mpsc::channel();
        tx.send(9).unwrap();
        drop(tx);
        let mut seen = Vec::new();
        assert_eq!(
            receive_batch(&rx, Duration::ZERO, |event| seen.push(event)),
            Ok(1)
        );
        assert_eq!(seen, [9]);
        assert_eq!(
            receive_batch(&rx, Duration::ZERO, |_| {}),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}

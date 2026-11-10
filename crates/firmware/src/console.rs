//! Serial command interface, mirroring the C prototype's keyboard commands.

use anyhow::Result;
use std::io::Read;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Next,
    Prev,
    Select(usize),
    Pair,
    Forget,
    Status,
    Help,
}

pub fn spawn(tx: Sender<Command>) -> Result<()> {
    thread::Builder::new()
        .name("console".into())
        .stack_size(4096)
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let mut byte = [0u8; 1];
            loop {
                match input.read(&mut byte) {
                    Ok(0) => thread::sleep(Duration::from_millis(100)),
                    Ok(_) => {
                        if let Some(command) = parse(byte[0])
                            && tx.send(command).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(100)),
                }
            }
        })?;
    Ok(())
}

fn parse(byte: u8) -> Option<Command> {
    match byte {
        b'n' => Some(Command::Next),
        b'b' => Some(Command::Prev),
        b'p' => Some(Command::Pair),
        b'X' => Some(Command::Forget),
        b's' => Some(Command::Status),
        b'h' | b'?' => Some(Command::Help),
        b'1'..=b'6' => Some(Command::Select(usize::from(byte - b'1'))),
        _ => None,
    }
}

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
    Probe,
    UsbRescan,
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
        b't' => Some(Command::Probe),
        b'u' => Some(Command::UsbRescan),
        b'h' | b'?' => Some(Command::Help),
        b'1'..=b'6' => Some(Command::Select(usize::from(byte - b'1'))),
        _ => None,
    }
}

pub fn print_help(slot_count: usize) {
    println!();
    println!("BT Switch (Rust) controls");
    println!(
        "  serial: n=next paired, b=previous paired, 1..{slot_count}=select slot, p=pair, X=forget, s=status, h=help"
    );
    println!("  t=send neutral HID probe to the selected host (no keys or movement)");
    println!("  u=restart USB root port and rescan (only when no HID interfaces are active)");
    #[cfg(feature = "official-otg")]
    {
        println!("  OK button: click=next slot, hold 3 s=pairing window");
        println!("  UP button: click=previous slot");
        println!("  DW button: click=pairing window");
        println!("  MENU button: click=status, hold 5 s=forget selected slot");
        println!(
            "  Green LED: ready solid / advertising blinking; yellow LED: USB receiver attached"
        );
    }
    #[cfg(feature = "prototype-button")]
    println!("  GPIO4 button: click=next paired slot, hold 3 s=pair; USB VBUS is hardware-managed");
    #[cfg(not(any(feature = "official-otg", feature = "prototype-button")))]
    println!(
        "  Generic board: buttons/LEDs disabled; USB Host requires external VBUS verification"
    );
    println!();
}

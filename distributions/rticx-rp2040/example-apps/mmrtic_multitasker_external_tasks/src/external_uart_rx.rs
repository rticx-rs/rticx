use core::sync::atomic::Ordering;

use fugit::MicrosDurationU32;
use heapless::String;
use rp2040_hal::timer::Alarm;

use crate::{
    app::{
        CommandReceiverTask, Decryptor, Encryptor, Hasher, RticMutex, RticTask, UartRx,
        TARGET_DURATION, TARGET_TICKS,
    },
    Command,
};

impl RticTask for CommandReceiverTask {
    type InitArgs = UartRx;
    fn init(uart_rx: UartRx) -> Self {
        Self {
            data: String::new(),
            read_command: true,
            command: Command::Unknown,
            uart_rx,
        }
    }

    fn exec(&mut self) {
        let mut data = [0_u8; 48];
        let bytes = self.uart_rx.read_raw(&mut data).unwrap();

        // echo back the read data
        self.shared()
            .uart_tx
            .lock(|uart| uart.write_full_blocking(&data[..bytes]));

        for b in &data[..bytes] {
            if self.read_command {
                // read command
                let cmd = match b {
                    b'b' => Command::Blink,
                    b'e' => Command::Encrypt,
                    b'd' => Command::Decrypt,
                    b'h' => Command::Hash,
                    _ => Command::Unknown,
                };
                self.command = cmd;
                self.read_command = false;
            } else if (b == &b'\n') || (b == &b'\r') {
                // command finished
                self.run_command();
                self.read_command = true;
                self.data.clear();
                self.command = Command::Unknown;
            } else if *b != b' ' || !self.data.is_empty() {
                // read command argument data
                let _ = self.data.push(*b as char);
            }
        }
    }
}

impl CommandReceiverTask {
    fn run_command(&mut self) {
        // command finished
        match self.command {
            Command::Blink => {
                // convert the buffers to values
                let (blinks, duration) = self.data.split_once(' ').unwrap_or(("0", "0"));

                let blinks: u32 = blinks.parse().unwrap_or(0);
                let duration: u32 = duration.parse().unwrap_or(0);
                TARGET_TICKS.store(blinks, Ordering::SeqCst);
                TARGET_DURATION.store(duration, Ordering::SeqCst);
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Starting blinky ...\r\n"));
                // start the first alarm
                self.shared().alarm.lock(|alarm| {
                    let _ = alarm.schedule(MicrosDurationU32::millis(duration));
                });
            }
            Command::Encrypt => {
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Starting encryptor ...\r\n"));
                let _ = Encryptor::spawn(self.data.clone());
            }
            Command::Decrypt => {
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Starting decryptor ...\r\n"));
                let _ = Decryptor::spawn(self.data.clone());
            }
            Command::Hash => {
                self.shared()
                    .uart_tx
                    .lock(|uart| uart.write_full_blocking(b"Starting hasher ...\r\n"));
                let _ = Hasher::spawn(self.data.clone());
            }

            Command::Unknown => self
                .shared()
                .uart_tx
                .lock(|uart| uart.write_full_blocking(b"Unknown command !\r\n")),
        }
    }
}

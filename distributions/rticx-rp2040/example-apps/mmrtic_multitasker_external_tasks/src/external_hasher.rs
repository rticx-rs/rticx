use cortex_m::asm;
use heapless::String;

use crate::app::{RticMutex, RticSwTask};

use super::app::Hasher;

impl RticSwTask for Hasher {
    type InitArgs = ();
    fn init(_: ()) -> Self {
        Self
    }

    type SpawnInput = String<30>;
    fn exec(&mut self, data: String<30>) {
        let hash = xor_hash(&data);
        let mut to_str = itoa::Buffer::new();
        let hash = to_str.format(hash);

        self.shared().uart_tx.lock(|uart| {
            uart.write_full_blocking(b"Hashing done: ");
            uart.write_full_blocking(hash.as_bytes());
            uart.write_full_blocking(b"\r\n");
        });
    }
}

fn xor_hash(data: &String<30>) -> u32 {
    let mut hash = 0u32;
    // XOR each byte into the 32-bit hash
    for (i, &byte) in data.as_bytes().iter().enumerate() {
        // Shift the byte into different positions within the u32 to spread out the effect
        let shift = (i % 4) * 8;
        hash ^= (byte as u32) << shift;
        asm::delay(1000); // simulate a more involved operation on each byte
    }
    hash
}

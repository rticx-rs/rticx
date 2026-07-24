use base64::{prelude::BASE64_STANDARD, Engine};
use cortex_m::asm;
use heapless::String;

use crate::app::{Decryptor, Encryptor, RticMutex, RticSwTask};

/// Random hardcode encryption key
const ENC_KEY: &[u8; 13] = b"fd@aG692-d70s";

fn xor_cipher(data: &mut [u8]) {
    for (i, byte) in data.iter_mut().enumerate() {
        let key_byte = ENC_KEY[i % ENC_KEY.len()]; // This wraps the key
        *byte ^= key_byte;
        asm::delay(1000); // simulate a more involved operation on each byte
    }
}

impl RticSwTask for Encryptor {
    type InitArgs = ();
    fn init(_: ()) -> Self {
        Self
    }

    type SpawnInput = String<30>;
    fn exec(&mut self, mut data: String<30>) {
        xor_cipher(unsafe { data.as_bytes_mut() });
        self.shared().uart_tx.lock(|uart| {
            uart.write_full_blocking(b"Encryption done: ");
            let mut out = [0; 100]; // 40 bytes are needed to represent 30 raw bytes in base64 format
            let size = base64::engine::general_purpose::STANDARD
                .encode_slice(data.as_bytes(), &mut out)
                .unwrap_or_default();
            uart.write_full_blocking(&out[..size]);
            uart.write_full_blocking(b"\r\n");
        });
    }
}

impl RticSwTask for Decryptor {
    type InitArgs = ();
    fn init(_: ()) -> Self {
        Self
    }

    type SpawnInput = String<30>;
    fn exec(&mut self, data: String<30>) {
        let mut out = [0; 100];
        let size = BASE64_STANDARD
            .decode_slice(data.as_bytes(), &mut out)
            .unwrap_or_default();
        xor_cipher(&mut out);
        self.shared().uart_tx.lock(|uart| {
            uart.write_full_blocking(b"Decryption done: ");
            uart.write_full_blocking(&out[..size]);
            uart.write_full_blocking(b"\r\n");
        });
    }
}

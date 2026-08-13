#![no_std]
#![no_main]

use esp_backtrace as _;
use riscv_semihosting::debug::EXIT_SUCCESS;

esp_bootloader_esp_idf::esp_app_desc!();

#[rticx_riscv::app(
    device = esp32c3,
    dispatchers = [FROM_CPU_INTR0, FROM_CPU_INTR1]
)]
mod app {
    use super::*;
    use esp_hal::delay;
    use esp_println::println;
    use rticx_async::channel::{Receiver, Sender};
    use rticx_async::make_channel;

    #[shared]
    struct Shared;

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        let _peripherals = esp_hal::init(esp_hal::Config::default());

        let (tx1, rx1) = make_channel!(u32, 4);
        let (tx2, rx2) = make_channel!(u32, 4);

        (
            Shared,
            TaskInits {
                ping: Ping { rx: rx1, tx: tx2 },
                pong: Pong { rx: rx2, tx: tx1 },
            },
        )
    }

    #[post_init]
    fn post_init() {
        let _ = Background::spawn(());
    }

    #[async_task(priority = 0, init = generated)]
    struct Background;
    impl RticAsyncTask for Background {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            for i in 1..=10 {
                println!("running at priority 0 each 500ms");
                delay::Delay::new().delay_millis(500);
                let _ = Periodic::spawn((i, 10));
            }
            println!("exiting");
            riscv_semihosting::debug::exit(EXIT_SUCCESS);
        }
    }

    #[async_task(priority = 2)]
    struct Ping {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Ping {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            println!("ping: sending 1 to pong");
            self.tx.send(1).await.expect("ping send must succeed");
            println!("ping: waiting reply from pong");
            let r = self.rx.recv().await.expect("ping recv must succeed");
            println!("ping: got {} from pong", r);
        }
    }

    #[async_task(priority = 2)]
    struct Pong {
        rx: Receiver<'static, u32, 4>,
        tx: Sender<'static, u32, 4>,
    }
    impl RticAsyncTask for Pong {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            println!("pong: waiting for ping to send something...");
            let r = self.rx.recv().await.expect("pong recv must succeed");
            println!("pong: got {} from ping, sending reply 7", r);
            self.tx.send(7).await.expect("pong send must succeed");
            println!("pong: done");
        }
    }

    #[async_task(priority = 3, init = generated)]
    struct Periodic;
    impl RticAsyncTask for Periodic {
        type SpawnInput = (i32, i32);
        async fn exec(&mut self, (i, count): (i32, i32)) {
            println!("\nperiodic task started");
            println!("[{}/{}]: Spawning lower prio tasks ping and pong", i, count);
            let _ = Pong::spawn(());
            let _ = Ping::spawn(());
            println!("Until next time...");
        }
    }
}

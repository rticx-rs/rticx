use core::sync::atomic::Ordering;

use embedded_hal::digital::v2::ToggleableOutputPin;
use fugit::MicrosDurationU32;
use rp2040_hal::timer::Alarm;

use crate::app::{CommandExecutorTask, LedPin, RticMutex, RticTask, TARGET_DURATION, TARGET_TICKS};

impl RticTask for CommandExecutorTask {
    type InitArgs = LedPin;
    fn init(led: LedPin) -> Self {
        Self { led }
    }

    fn exec(&mut self) {
        let duration = TARGET_DURATION.load(Ordering::SeqCst);
        let blinks_left = TARGET_TICKS.load(Ordering::SeqCst);
        let blinks_left = blinks_left.saturating_sub(1);
        TARGET_TICKS.store(blinks_left, Ordering::SeqCst);

        // toggle the LED
        let _ = self.led.toggle();

        if blinks_left == 0 {
            self.shared()
                .uart_tx
                .lock(|uart| uart.write_full_blocking(b"finished pattern !\r\n"));
        }

        // don't forget to clear the interrrupt
        self.shared().alarm.lock(|alarm0| {
            if blinks_left != 0 {
                let _ = alarm0.schedule(MicrosDurationU32::millis(duration));
            }
            alarm0.clear_interrupt();
        });
    }
}

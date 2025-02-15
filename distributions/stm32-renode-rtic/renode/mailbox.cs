using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals;
using Antmicro.Renode.Utilities;
using Antmicro.Renode.Peripherals.Bus;

namespace MMRtic {

    public class MailboxPeripheral : IKnownSize, IDoubleWordPeripheral, IMailbox
    {
        private uint statusRegister = 0;
        private Queue<uint> queue;
        private int queue_depth;
        private Machine machine;
        private IMailbox rx = null;

        // public GPIO irq;
        public GPIO irq { get; private set; }

        // Register map
        public enum Registers : long
        {
            FIFO_RD = 0x00, // FIFO read register
            FIFO_WR = 0x04, // FIFO write register
            FIFO_ST = 0x08  // FIFO status register
        }

        // status register is like this
        // [0 | 0 | .. 0(26).. | 0 | A | E | F ] 
        // Status register bit masks
        private const uint FLAG_QUEUE_FULL = 1u << 0; // F flag
        private const uint FLAG_QUEUE_EMPTY = 1u << 1; // E flag
        private const uint FLAG_DATA_AVAILABLE = 1u << 2; // A flag

        public MailboxPeripheral(Machine machine, uint depth = 8)
        {
            this.machine = machine;
            this.queue_depth = (int)depth;
            this.queue = new Queue<uint>(this.queue_depth);
            this.irq = new GPIO();
        }

        /// reset signal, puts the peripheral in intitial state 
        public void Reset()
        {
            this.statusRegister = FLAG_QUEUE_EMPTY; // Set Empty Bit in the status register
            this.queue.Clear();
            this.irq.Unset();
        }

        /// Read from the mailbox (if not empty)
        public uint ReadDoubleWord(long offset)
        {
            switch (offset)
            {
                case (long)Registers.FIFO_RD: // read register
                    return ReadRegister;
                case (long)Registers.FIFO_ST: // status register
                    // first update info about other fifo fullness
                    if (this.rx.IsFull()) {
                        // inform that sending other messages will be discarded untill the other mailbox queue has some free space
                        statusRegister |= FLAG_QUEUE_FULL; 
                    } else {
                        // inform that sending other messages is possible since the other mailbox queue has some free space
                        statusRegister &= ~FLAG_QUEUE_FULL;
                    }
                    return statusRegister;
                default:
                    this.Log(LogLevel.Warning, $"MailboxPeripheral: Attempted to read from an invalid offset: 0x{offset:X}");
                    return 0;
            }
        }

        /// Write to the mailbox (if not full)
        public void WriteDoubleWord(long offset, uint value)
        {
            switch (offset)
            {
                case (long)Registers.FIFO_WR:
                    // try to push data to the queue other mailbox
                    this.rx.OnPush(value);
                    break;
                default:
                    this.Log(LogLevel.Warning, $"MailboxPeripheral: Attempted to write to an invalid offset: 0x{offset:X}");
                    break;
            }
        }

        // will be called by the other mailbox to push data into this queue
        public void OnPush(uint value) {
            this.Log(LogLevel.Info, $"got message: {value}");
            // only push data if queue is not full
            // in such case also inform data is available to read
            if (queue.Count != queue_depth)
            {
                statusRegister |= FLAG_DATA_AVAILABLE; // inform that data is available
                queue.Enqueue(value); // push
                irq.Set();  // Set interrupt when data is written to the queue
            }
        }

        public uint ReadRegister
        {
            get
            {
                if (queue.Count > 0)
                {   
                    uint readRegister = queue.Dequeue(); // pop
                    this.Log(LogLevel.Info, $"reading from mailbox: {readRegister}");
                    // set empty status bit and clear available bit if queue is drained
                    if (queue.Count == 0 ) 
                    {
                        statusRegister |= FLAG_QUEUE_EMPTY;
                        statusRegister &= ~FLAG_DATA_AVAILABLE;
                        irq.Unset();  // Unset interrupt when data is read from the queue
                    }
                    return readRegister;
                }
                else // queue empty
                {
                    return 0;
                }
            }
        }

        // Implement IKnownSize interface method
        public long Size
        {
            get
            {
                return 0x12;
            }
        }
    
        // set the other mailbox as receiver
        public void SetRx(IMailbox rx) {
            this.rx = rx;
        }

        public bool IsFull() {
            return this.queue.Count == this.queue_depth;
        }
    }

    // Interface for cross-mailbox communication
    public interface IMailbox
    {   
        // other mailbox will call this function to push data
        void OnPush(uint data);
        // sets the other mailbox as a receiver
        void SetRx(IMailbox rx);
        // information about fullness of internal queue
        bool IsFull(); 
    }


    // A proxy is needed to provide cross referencing between the the two mirror mailboxes 
    public class MailboxProxy: Antmicro.Renode.IEmulationElement
    {
        public MailboxProxy(Machine machine, IMailbox m0,  IMailbox m1)
        {
            m0.SetRx(m1);
            m1.SetRx(m0);
        }
    }

}


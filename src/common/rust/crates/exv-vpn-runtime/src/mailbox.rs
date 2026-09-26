
use std::collections::VecDeque;

/// 有界邮箱满时拒绝消息，返回持有被拒消息的 `MailboxFull`，便于调用方重试或改道。
#[derive(Debug, PartialEq, Eq)]
pub struct MailboxFull<T>(pub T);

/// 固定容量的 FIFO 邮箱。
///
/// - `try_send`：满时拒绝，返回 `Err(MailboxFull(rejected))`，被拒消息不会被缓冲。
/// - `try_send_drop_oldest`：满时淘汰最旧消息并返回它，新消息入队（latest-wins）；未满时入队并返回 `None`。
/// - `try_recv`：按 FIFO 顺序取出一条消息。
pub struct BoundedMailbox<T> {
    capacity: usize,
    items: VecDeque<T>,
}

impl<T> BoundedMailbox<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: VecDeque::with_capacity(capacity),
        }
    }

    pub fn try_send(&mut self, item: T) -> Result<(), MailboxFull<T>> {
        if self.items.len() >= self.capacity {
            return Err(MailboxFull(item));
        }
        self.items.push_back(item);
        Ok(())
    }

    pub fn try_send_drop_oldest(&mut self, item: T) -> Option<T> {
        if self.items.len() >= self.capacity {
            let dropped = self.items.pop_front();
            self.items.push_back(item);
            return dropped;
        }
        self.items.push_back(item);
        None
    }

    pub fn try_recv(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }
}


//! Random messages for the fuzz tests of the session.
//!
//! Plain random bytes rarely name anything that exists, so a handler would mostly answer "no such
//! handle". This remembers what replies gave out and mixes that into later messages, along with
//! numbers that sit on the edges of what fits.

use guesthouse_backend::WINDOW_BASE;
use guesthouse_messages::wire::{set_u32, set_u64};
use guesthouse_messages::{Command, HEADER_SIZE};

/// Where a message keeps its process and its command.
const PROCESS: usize = 8;
const COMMAND: usize = 16;

/// Enough to reach every fixed field, the furthest being a process's flags at 0x232.
const USUAL_BODY: usize = 0x240;
/// Now and then a longer message, for lists and private data.
const LONG_BODY: usize = 0x4000;
const ONE_IN_LONG: u64 = 16;

/// The most fields changed in a message made from a template.
const MOST_CHANGES: usize = 6;

const REMEMBERED: usize = 256;
const HANDLES: std::ops::Range<u64> = 0x4000_0000..0x4100_0000;
/// Replies hold GPU and guest physical addresses, both below this.
const HIGHEST_ADDRESS: u64 = 1 << 48;

const EDGES: [u64; 14] = [
    1,
    2,
    8,
    0x1000,
    0x10000,
    0x7FFF_FFFF,
    0xFFFF_FFFF,
    0x1_0000_0000,
    1 << 39,
    WINDOW_BASE,
    u64::MAX / 2,
    u64::MAX - 0xFFF,
    u64::MAX - 1,
    u64::MAX,
];

/// See the module documentation. The same seed gives the same messages, for the same replies.
#[derive(Debug)]
pub struct RandomMessages {
    state: u64,
    commands: Vec<u32>,
    templates: Vec<Vec<u8>>,
    remembered: Vec<u64>,
    next_slot: usize,
}

impl RandomMessages {
    /// `seed` must not be zero. `commands` is what to send, by number on the wire.
    pub fn new(seed: u64, commands: Vec<u32>) -> Self {
        Self { state: seed, commands, templates: Vec::new(), remembered: Vec::new(), next_slot: 0 }
    }

    /// Sets well-formed messages to start from, as about half of what comes next. A message made of
    /// random fields gets no further than the first check that matters, and a good one with a few
    /// fields changed gets past it.
    pub fn set_templates(&mut self, templates: Vec<Vec<u8>>) {
        self.templates = templates;
    }

    /// Every command the session knows, and a few numbers it doesn't.
    pub fn all_commands() -> Vec<u32> {
        let known = (0..=1100).filter(|&number| !matches!(Command::from_wire(number), Command::Unknown(_)));
        known.chain([67, 4242, u32::MAX]).collect()
    }

    /// The next number from an xorshift, which is all a test needs.
    pub fn number(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.number() % limit as u64) as usize
    }

    fn remembered_value(&mut self) -> u64 {
        if self.remembered.is_empty() {
            return 0;
        }
        let index = self.below(self.remembered.len());
        self.remembered[index]
    }

    fn field_value(&mut self) -> u64 {
        match self.below(8) {
            0..=2 => 0,
            3 | 4 => self.remembered_value(),
            5 => EDGES[self.below(EDGES.len())],
            6 => self.number() % 0x100,
            _ => self.number(),
        }
    }

    /// A message: a template with a few fields changed, or one of random fields.
    pub fn message(&mut self) -> Vec<u8> {
        if !self.templates.is_empty() && self.number().is_multiple_of(2) {
            self.changed_template()
        } else {
            self.random_message()
        }
    }

    fn changed_template(&mut self) -> Vec<u8> {
        let template = self.below(self.templates.len());
        let mut message = self.templates[template].clone();
        let fields = (message.len() - HEADER_SIZE) / size_of::<u32>();
        if fields == 0 {
            return message;
        }
        for _ in 0..self.below(MOST_CHANGES + 1) {
            let at = HEADER_SIZE + self.below(fields) * size_of::<u32>();
            let value = self.field_value();
            set_u32(&mut message, at, value as u32);
        }
        message
    }

    fn random_message(&mut self) -> Vec<u8> {
        let body = if self.number().is_multiple_of(ONE_IN_LONG) { LONG_BODY } else { USUAL_BODY };
        let mut message = vec![0; HEADER_SIZE + self.below(body)];
        let mut at = 0;
        while at + size_of::<u64>() <= message.len() {
            let value = self.field_value();
            if self.number().is_multiple_of(2) {
                set_u64(&mut message, at, value);
                at += size_of::<u64>();
            } else {
                set_u32(&mut message, at, value as u32);
                at += size_of::<u32>();
            }
        }
        let command = self.below(self.commands.len());
        set_u32(&mut message, COMMAND, self.commands[command]);
        let process = self.remembered_value() as u32 % 4;
        set_u32(&mut message, PROCESS, process);
        message
    }

    fn remember(&mut self, value: u64) {
        if self.remembered.len() < REMEMBERED {
            self.remembered.push(value);
        } else {
            self.remembered[self.next_slot] = value;
            self.next_slot = (self.next_slot + 1) % REMEMBERED;
        }
    }

    /// Picks out what looks like a handle or an address in a reply, to use in later messages.
    pub fn learn(&mut self, reply: &[u8]) {
        let (words, _) = reply.as_chunks::<8>();
        for word in words.iter().map(|word| u64::from_le_bytes(*word)) {
            let (low, high) = (word & 0xFFFF_FFFF, word >> 32);
            if HANDLES.contains(&low) {
                self.remember(low);
            }
            if HANDLES.contains(&high) {
                self.remember(high);
            }
            if word != 0 && word < HIGHEST_ADDRESS && !HANDLES.contains(&word) {
                self.remember(word);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_messages::wire::u32_at;

    #[test]
    fn every_message_names_a_command_it_was_given() {
        let mut messages = RandomMessages::new(1, vec![7]);
        for _ in 0..100 {
            assert_eq!(u32_at(&messages.message(), COMMAND), Ok(7));
        }
    }

    #[test]
    fn templates_keep_their_command_and_length() {
        let mut messages = RandomMessages::new(1, vec![7]);
        let mut template = vec![0; HEADER_SIZE + 16];
        set_u32(&mut template, COMMAND, 9);
        messages.set_templates(vec![template]);
        let from_template = (0..100).map(|_| messages.message()).filter(|message| message[COMMAND] == 9);
        assert!(from_template.into_iter().all(|message| message.len() == HEADER_SIZE + 16));
    }

    #[test]
    fn what_a_reply_gave_out_comes_back() {
        let mut messages = RandomMessages::new(1, vec![7]);
        messages.learn(&0x4000_1040u64.to_le_bytes());
        let found = (0..100).any(|_| {
            let message = messages.message();
            message.as_chunks::<4>().0.iter().any(|word| u32::from_le_bytes(*word) == 0x4000_1040)
        });
        assert!(found);
    }
}

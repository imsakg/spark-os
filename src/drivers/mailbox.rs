//! BCM property-message construction and explicitly unsafe firmware transactions.

use super::mmio::RegisterIo;
const RESPONSE: u32 = 1 << 31;

/// Message construction, protocol, or bounded polling failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxError {
    /// The fixed word buffer cannot hold the requested message.
    Capacity,
    /// A zero tag ID, undersized value buffer, or unrepresentable length.
    InvalidTag,
    /// Firmware did not return the global success response code.
    FirmwareRejected,
    /// Header, terminator, or word-based tag framing is invalid.
    MalformedResponse,
    /// Firmware reports a desired response length larger than supplied capacity.
    /// This is valid truncation on the wire, but this strict API refuses it.
    TruncatedResponse,
    /// Firmware left this tag's response bit unset (for example, an unknown tag).
    UnsupportedTag(u32),
    /// The requested tag ID was absent from a valid response.
    MissingTag,
    /// Bus address is zero or not 16-byte aligned.
    InvalidAddress,
    /// No request was written before the polling budget expired.
    SendTimeout,
    /// A request was sent; firmware may still own and mutate its buffer.
    ReplyTimeout,
}

/// Aligned property-message storage; only `words()` is part of the wire format.
///
/// Bus addresses are never inferred from a Rust pointer. The platform must
/// translate/pin the buffer, manage caches, and establish firmware visibility.
#[repr(C, align(16))]
pub struct PropertyMessage<const WORDS: usize> {
    words: [u32; WORDS],
    used: usize,
}
impl<const WORDS: usize> PropertyMessage<WORDS> {
    /// Start with size, request status, and terminating zero tag.
    pub fn new() -> Result<Self, MailboxError> {
        if WORDS < 3 || WORDS > u32::MAX as usize / 4 {
            return Err(MailboxError::Capacity);
        }
        let mut message = Self {
            words: [0; WORDS],
            used: 3,
        };
        message.words[0] = 12;
        Ok(message)
    }
    /// Append a word-based tag, reserving enough value words for its response.
    /// Failed construction leaves the previous message unchanged.
    pub fn add_tag(
        &mut self,
        id: u32,
        value_words: usize,
        request: &[u32],
    ) -> Result<(), MailboxError> {
        if id == 0 || request.len() > value_words || value_words > (u32::MAX >> 1) as usize / 4 {
            return Err(MailboxError::InvalidTag);
        }
        let next = self
            .used
            .checked_add(3)
            .and_then(|size| size.checked_add(value_words))
            .ok_or(MailboxError::Capacity)?;
        if next > WORDS {
            return Err(MailboxError::Capacity);
        }
        let tag = self.used - 1;
        self.words[tag] = id;
        self.words[tag + 1] = (value_words * 4) as u32;
        // Request bits 30:0 are reserved, not a request-value length.
        self.words[tag + 2] = 0;
        let values = tag + 3;
        self.words[values..values + value_words].fill(0);
        self.words[values..values + request.len()].copy_from_slice(request);
        self.used = next;
        self.words[next - 1] = 0;
        self.words[0] = (next * 4) as u32;
        Ok(())
    }
    /// Header, tags, and terminating word, excluding unused capacity/metadata.
    pub fn words(&self) -> &[u32] {
        &self.words[..self.used]
    }
    /// Mutable wire words for explicit protocol handling or host-test replies.
    /// Do not mutate or create references while firmware owns the buffer.
    pub fn words_mut(&mut self) -> &mut [u32] {
        &mut self.words[..self.used]
    }
    /// Get the aligned CPU pointer; this is NOT a firmware bus address.
    pub fn as_mut_ptr(&mut self) -> *mut u32 {
        self.words.as_mut_ptr()
    }
    /// Validate the global response and every word-based tag's framing/length.
    /// This is structural validation, not authentication or payload validation.
    pub fn validate_response(&self) -> Result<(), MailboxError> {
        if self.words[0] != (self.used * 4) as u32 {
            return Err(MailboxError::MalformedResponse);
        }
        if self.words[1] != RESPONSE {
            return Err(MailboxError::FirmwareRejected);
        }
        let mut tag = 2;
        while tag < self.used {
            if self.words[tag] == 0 {
                return if tag == self.used - 1 {
                    Ok(())
                } else {
                    Err(MailboxError::MalformedResponse)
                };
            }
            if tag + 3 >= self.used {
                return Err(MailboxError::MalformedResponse);
            }
            let capacity = self.words[tag + 1] as usize;
            let response = self.words[tag + 2];
            let length = (response & !RESPONSE) as usize;
            if capacity % 4 != 0 {
                return Err(MailboxError::MalformedResponse);
            }
            if response & RESPONSE == 0 {
                return Err(MailboxError::UnsupportedTag(self.words[tag]));
            }
            if length > capacity {
                return Err(MailboxError::TruncatedResponse);
            }
            if length % 4 != 0 {
                return Err(MailboxError::MalformedResponse);
            }
            tag = (tag + 3)
                .checked_add(capacity / 4)
                .ok_or(MailboxError::MalformedResponse)?;
        }
        Err(MailboxError::MalformedResponse)
    }
    /// Find a caller-expected tag ID after complete structural validation.
    /// Returned words exclude unused value-buffer capacity.
    pub fn response(&self, id: u32) -> Result<&[u32], MailboxError> {
        self.validate_response()?;
        let mut tag = 2;
        while self.words[tag] != 0 {
            if self.words[tag] == id {
                let count = (self.words[tag + 2] & !RESPONSE) as usize / 4;
                return Ok(&self.words[tag + 3..tag + 3 + count]);
            }
            tag += 3 + self.words[tag + 1] as usize / 4;
        }
        Err(MailboxError::MissingTag)
    }
}

/// Exclusive ARM mailbox endpoint based at MAIL0_READ.
pub struct Mailbox<T: RegisterIo> {
    io: T,
}
impl<T: RegisterIo> Mailbox<T> {
    /// Bind a mailbox transport without issuing a firmware request.
    pub fn new(io: T) -> Self {
        Self { io }
    }
    /// Recover register ownership; does not cancel an in-flight request.
    pub fn release(self) -> T {
        self.io
    }
    /// Submit channel-eight properties, matching the complete reply token.
    /// `polls` bounds the total send/receive status reads, not elapsed time.
    ///
    /// # Safety
    /// The bus address must name pinned, aligned, valid property-message memory
    /// exclusively owned by the caller and firmware. Set required DMA barriers
    /// and clean caches before this call; invalidate/synchronize after success
    /// before accessing a response. Never free, move, reuse, or reference the
    /// buffer while firmware might write it, INCLUDING after `ReplyTimeout`.
    /// Quarantine timed-out storage until platform-specific recovery proves
    /// completion. Own the entire receive mailbox: unrelated tokens are consumed
    /// while matching, so no other channel may have an outstanding transaction.
    /// A matching token alone does not validate the property-message contents.
    pub unsafe fn property_call(
        &mut self,
        bus_address: u32,
        mut polls: usize,
    ) -> Result<(), MailboxError> {
        if bus_address == 0 || bus_address & 15 != 0 {
            return Err(MailboxError::InvalidAddress);
        }
        let token = bus_address | 8;
        let mut sent = false;
        while polls != 0 {
            polls -= 1;
            if !sent {
                if self.io.read32(0x38) & (1 << 31) == 0 {
                    self.io.write32(0x20, token);
                    sent = true;
                }
            } else if self.io.read32(0x18) & (1 << 30) == 0 && self.io.read32(0) == token {
                return Ok(());
            }
        }
        Err(if sent {
            MailboxError::ReplyTimeout
        } else {
            MailboxError::SendTimeout
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::test_support::{
        Access::{Read, Write},
        FakeIo,
    };
    #[test]
    fn request_builder_bounds_capacity_and_validates_per_tag_reply_lengths() {
        let mut message = PropertyMessage::<12>::new().unwrap();
        message.add_tag(0x30002, 3, &[2]).unwrap();
        assert_eq!(message.words(), &[36, 0, 0x30002, 12, 0, 2, 0, 0, 0]);
        assert_eq!((message.as_mut_ptr() as usize) % 16, 0);
        let before = message.words().to_vec();
        assert_eq!(message.add_tag(1, 2, &[]), Err(MailboxError::Capacity));
        assert_eq!(message.words(), before.as_slice());
        message.words_mut()[1] = 0x80000000;
        message.words_mut()[4] = 0x8000000c;
        assert_eq!(message.response(0x30002), Ok(&[2, 0, 0][..]));
        assert_eq!(message.response(0x10001), Err(MailboxError::MissingTag));
        message.words_mut()[4] = 0x80000010;
        assert_eq!(
            message.validate_response(),
            Err(MailboxError::TruncatedResponse)
        );
        message.words_mut()[4] = 12;
        assert_eq!(
            message.validate_response(),
            Err(MailboxError::UnsupportedTag(0x30002))
        );
        assert!(matches!(
            PropertyMessage::<2>::new(),
            Err(MailboxError::Capacity)
        ));
    }
    #[test]
    fn transaction_polls_correct_fifos_and_matches_full_address_and_channel() {
        let mut mailbox = Mailbox::new(FakeIo::new([
            Read(0x38, 1 << 31),
            Read(0x38, 0),
            Write(0x20, 0x1008),
            Read(0x18, 1 << 30),
            Read(0x18, 0),
            Read(0, 0x2008),
            Read(0x18, 0),
            Read(0, 0x1008),
        ]));
        // Fake transport: no firmware/DMA is involved in this host test.
        assert_eq!(unsafe { mailbox.property_call(0x1000, 5) }, Ok(()));
        mailbox.release().done();
    }
    #[test]
    fn timeout_distinguishes_not_sent_from_possible_inflight_dma() {
        let mut mailbox = Mailbox::new(FakeIo::new([Read(0x38, 0), Write(0x20, 0x1008)]));
        assert_eq!(
            unsafe { mailbox.property_call(0x1001, 9) },
            Err(MailboxError::InvalidAddress)
        );
        assert_eq!(
            unsafe { mailbox.property_call(0x1000, 0) },
            Err(MailboxError::SendTimeout)
        );
        assert_eq!(
            unsafe { mailbox.property_call(0x1000, 1) },
            Err(MailboxError::ReplyTimeout)
        );
        mailbox.release().done();
    }
}

#[cfg(test)]
mod malformed_response_regressions {
    use super::*;
    #[test]
    fn hostile_header_fields_and_lengths_are_rejected_without_panics() {
        for index in 0..9 {
            for value in [0, 1, 3, 4, 0x80000000, 0x80000004, u32::MAX] {
                let mut message = PropertyMessage::<12>::new().unwrap();
                message.add_tag(0x10001, 3, &[]).unwrap();
                message.words_mut()[1] = 0x80000000;
                message.words_mut()[4] = 0x80000004;
                message.words_mut()[index] = value;
                // No memory access is made through returned lengths until the
                // complete response has passed structural validation.
                if message.validate_response().is_ok() {
                    let _ = message.response(0x10001);
                }
            }
        }
    }
}

//! Adaptive 40-80ms jitter buffer with a bounded reorder window for 2.5–120ms Opus packets.
const SLOTS: usize = 16;
const MIN_WAIT: u8 = 2;
const MAX_WAIT: u8 = 4;

pub(crate) struct Jitter {
	packets: Vec<(u16, Vec<u8>)>,
	next: Option<u16>,
	wait: u8,
	target_wait: u8,
	steady: u16,
	missing: u8,
}

impl Default for Jitter {
	fn default() -> Self {
		Self {
			packets: Vec::new(),
			next: None,
			wait: 0,
			target_wait: MIN_WAIT,
			steady: 0,
			missing: 0,
		}
	}
}

impl Jitter {
	pub fn clear(&mut self) {
		let target_wait = self.target_wait;
		*self = Self {
			target_wait,
			..Self::default()
		};
	}
	pub fn push(&mut self, sequence: u16, opus: Vec<u8>) {
		if opus.len() > 1275 {
			return;
		}
		if self.next.is_none() {
			self.next = Some(sequence);
			self.wait = self.target_wait;
		}
		let next = self.next.unwrap();
		let distance = sequence.wrapping_sub(next);
		if distance >= 32768 {
			return;
		}
		if distance > 1 && distance < SLOTS as u16 {
			// Out-of-order/burst arrival seen: raise target wait gently on the next reset.
			self.target_wait = self.target_wait.max(3);
			self.steady = 0;
		}
		if distance >= SLOTS as u16 * 4 {
			self.packets.clear();
			self.next = Some(sequence);
			self.target_wait = MIN_WAIT;
			self.wait = self.target_wait;
			self.missing = 0;
			self.steady = 0;
		} else if distance >= SLOTS as u16 {
			// Moderate burst jump: slide `next` forward instead of wiping valid ahead frames.
			let new_next = sequence.wrapping_sub((SLOTS - 1) as u16);
			self.packets
				.retain(|(id, _)| id.wrapping_sub(new_next) < SLOTS as u16);
			self.next = Some(new_next);
			self.missing = 0;
			self.target_wait = MAX_WAIT;
			self.steady = 0;
		}
		if self.packets.len() < SLOTS && !self.packets.iter().any(|(id, _)| *id == sequence) {
			self.packets.push((sequence, opus));
		}
	}
	/// An empty packet requests Opus packet-loss concealment, at most three consecutive packets.
	pub fn pop(&mut self) -> Option<Vec<u8>> {
		if self.wait > 0 {
			// Short packets can fill the fixed window before startup finishes. Start before
			// the next arrival would repeatedly reset a full window.
			if self.packets.len() < SLOTS {
				self.wait -= 1;
				return None;
			}
			self.wait = 0;
		}
		let next = self.next?;
		self.next = Some(next.wrapping_add(1));
		if let Some(index) = self.packets.iter().position(|(id, _)| *id == next) {
			self.missing = 0;
			self.steady = self.steady.saturating_add(1);
			if self.steady >= 250 && self.target_wait > MIN_WAIT {
				self.target_wait -= 1;
				self.steady = 0;
			}
			return Some(self.packets.swap_remove(index).1);
		}
		self.missing += 1;
		self.steady = 0;
		if self.missing > 3 {
			// Stop concealing the gap, but retain speech that already arrived after it.
			if let Some((index, (sequence, _))) = self
				.packets
				.iter()
				.enumerate()
				.min_by_key(|(_, (sequence, _))| sequence.wrapping_sub(next))
			{
				self.next = Some(sequence.wrapping_add(1));
				self.missing = 0;
				self.target_wait = (self.target_wait + 1).min(MAX_WAIT);
				return Some(self.packets.swap_remove(index).1);
			}
			// Buffer underran after an unrecovered gap: keep extra wait room if jittery.
			if self.target_wait < MAX_WAIT {
				self.target_wait += 1;
			}
			self.clear();
			return None;
		}
		Some(Vec::new())
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn burst_loss_preserves_buffered_recovery_and_resets_on_far_jump() {
		for first in [0u16, u16::MAX - 2] {
			let mut jitter = Jitter::default();
			jitter.push(first, vec![0]);
			for offset in [7, 5, 6] {
				jitter.push(first.wrapping_add(offset), vec![offset as u8]);
			}
			assert!(jitter.pop().is_none());
			assert!(jitter.pop().is_none());
			assert_eq!(jitter.pop(), Some(vec![0]));
			for _ in 0..3 {
				assert_eq!(jitter.pop(), Some(vec![]));
			}
			for offset in [5, 6, 7] {
				assert_eq!(jitter.pop(), Some(vec![offset]));
			}
			for _ in 0..2 {
				assert_eq!(jitter.pop(), Some(vec![]));
			}
			jitter.push(first.wrapping_add(100), vec![100]);
			assert_eq!(jitter.missing, 0);
			assert!(jitter.pop().is_none());
			assert!(jitter.pop().is_none());
			assert_eq!(jitter.pop(), Some(vec![100]));
		}
	}
	#[test]
	fn bounded_reordering_loss_duplicates_and_wrap() {
		let mut jitter = Jitter::default();
		jitter.push(u16::MAX, vec![1]);
		jitter.push(1, vec![3]);
		jitter.push(0, vec![2]);
		jitter.push(0, vec![9]);
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		assert_eq!(jitter.pop(), Some(vec![1]));
		assert_eq!(jitter.pop(), Some(vec![2]));
		assert_eq!(jitter.pop(), Some(vec![3]));
		for _ in 0..3 {
			assert_eq!(jitter.pop(), Some(vec![]));
		}
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		for sequence in 0..1000 {
			jitter.push(sequence, vec![1; 1275]);
			assert!(jitter.packets.len() <= SLOTS);
		}
		jitter.push(1000, vec![0; 1276]);
		assert!(jitter.packets.len() <= SLOTS);
	}
	#[test]
	fn moderate_burst_slides_window_without_wiping_ahead_frames() {
		let mut jitter = Jitter::default();
		jitter.push(0, vec![0]);
		// Burst jump within 4x window keeps recent ahead frames intact.
		for seq in 17..=20 {
			jitter.push(seq, vec![seq as u8]);
		}
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		let mut recovered = Vec::new();
		for _ in 0..20 {
			if let Some(pkt) = jitter.pop()
				&& !pkt.is_empty()
			{
				recovered.push(pkt[0]);
			}
		}
		assert_eq!(recovered, vec![17, 18, 19, 20]);
	}
}

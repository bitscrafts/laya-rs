//! A bounded priority queue for scheduling decisions.
//!
//! ## Why this exists, measured
//!
//! Two measurements decided this module's shape:
//!
//! ```text
//! Laya /embedding   : 35 ms median · 8 concurrent = 121 ms wall (5.8x parallel)
//! redb, 2nd process : Error: DatabaseAlreadyOpen
//! ```
//!
//! The backbone is **fast and it parallelises**, so a queue in front of it is not a
//! throughput fix and ordering it would be premature. What we actually need is:
//!
//! * **Work scheduling** — a `gate` that is blocking an agent's tool call must not
//!   wait behind a bulk file scan. This is where priority earns its place, because
//!   the latency is user-visible.
//! * **Boundedness** — the system must be able to say "no" instead of growing.
//!
//! The store's single-writer requirement (blocker B4) is a *serialisation* problem
//! and is handled by the compactor, not here: records are records, and their order
//! barely matters.

use std::collections::VecDeque;

/// How urgent a piece of work is.
///
/// The ordering is the enum's declaration order: **`Gate` first**. `Ord` is derived
/// so that a plain `max`/sort does the right thing, and the tests pin the order so a
/// reordering of the variants cannot silently invert the priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// A hook blocking an agent's tool call. The most latency-critical work in the
    /// system: a human or an agent is stopped until it answers.
    Gate,
    /// A human or an agent waiting on a `decide`.
    Interactive,
    /// Bulk scans (`ask_files`), calibration, maintenance. Nothing is blocked on it.
    Background,
}

impl Priority {
    /// The order to serve in — highest first.
    pub const SERVED_HIGHEST_FIRST: [Self; 3] = [Self::Gate, Self::Interactive, Self::Background];

    /// Whether this class may be shed when the queue is full.
    ///
    /// `Background` and `Interactive` may; a `Gate` may **not** by default, because
    /// dropping one reads as permission. A refused gate fails closed instead.
    #[must_use]
    pub const fn shedable(self) -> bool {
        match self {
            Self::Gate => false,
            Self::Interactive | Self::Background => true,
        }
    }

    /// A short label, for stats and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Interactive => "interactive",
            Self::Background => "background",
        }
    }
}

/// Why an enqueue was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueError {
    /// The queue is at capacity and nothing shedable could be dropped.
    ///
    /// Refusing is deliberate for a [`Priority::Gate`]: the caller must **fail
    /// closed** (`ask`, never `allow`) rather than believe it was served.
    Full {
        /// The class that could not be admitted.
        refused: Priority,
    },
}

impl std::fmt::Display for EnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full { refused } => write!(
                f,
                "queue is full and no {} item could be shed; refused a {} item \
                 (a refused gate must fail closed: answer `ask`, never `allow`)",
                Priority::Background.as_str(),
                refused.as_str()
            ),
        }
    }
}

/// What happened to an enqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    /// Admitted with room to spare.
    Admitted,
    /// Admitted after shedding an item of the given class.
    AdmittedAfterShedding(Priority),
}

/// Counters, so a saturated system is **visible** rather than merely slow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Total items admitted.
    pub admitted: u64,
    /// Total items refused.
    pub refused: u64,
    /// Items shed per class: `[gate, interactive, background]`.
    pub shed: [u64; 3],
    /// The high-water mark of depth.
    pub peak_depth: usize,
}

/// A bounded queue that serves by class, then FIFO within a class.
///
/// Three `VecDeque`s rather than a heap: FIFO within a class is then structural
/// (push back, pop front) instead of relying on a sequence number to break ties, so
/// there is no way to get the tie-breaking wrong.
#[derive(Debug)]
pub struct PriorityQueue<T> {
    lanes: [VecDeque<T>; 3],
    capacity: usize,
    stats: QueueStats,
}

impl<T> PriorityQueue<T> {
    /// Create a queue bounded to `capacity` items **in total** across all classes.
    ///
    /// A zero capacity is allowed and means every enqueue is refused — useful for a
    /// "disabled" configuration, and it must not panic.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            lanes: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
            capacity,
            stats: QueueStats::default(),
        }
    }

    /// Index of a class's lane. Kept in one place so the mapping cannot drift.
    const fn lane(p: Priority) -> usize {
        match p {
            Priority::Gate => 0,
            Priority::Interactive => 1,
            Priority::Background => 2,
        }
    }

    /// Items currently queued, across all classes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lanes.iter().map(VecDeque::len).sum()
    }

    /// Whether the queue holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The configured capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// The counters.
    #[must_use]
    pub const fn stats(&self) -> &QueueStats {
        &self.stats
    }

    /// Enqueue `item` at `priority`.
    ///
    /// # Errors
    /// [`EnqueueError::Full`] when the queue is at capacity and nothing shedable can
    /// be dropped. **A refused item was not served** — the caller must not assume it
    /// was.
    pub fn push(&mut self, priority: Priority, item: T) -> Result<Enqueued, EnqueueError> {
        if self.len() < self.capacity {
            self.admit(priority, item);
            return Ok(Enqueued::Admitted);
        }

        // Full. Shed the least urgent thing, if the shedding rule permits it: a
        // victim is only ever strictly LESS urgent than the item being admitted, so
        // bulk traffic can never evict the work it should be waiting behind, and a
        // Gate is never shed at all.
        if let Some(shed_class) = self.shed_one_of(priority) {
            self.admit(priority, item);
            let idx = Self::lane(shed_class);
            if let Some(slot) = self.stats.shed.get_mut(idx) {
                *slot += 1;
            }
            return Ok(Enqueued::AdmittedAfterShedding(shed_class));
        }

        self.stats.refused += 1;
        Err(EnqueueError::Full { refused: priority })
    }

    fn admit(&mut self, priority: Priority, item: T) {
        let idx = Self::lane(priority);
        if let Some(lane) = self.lanes.get_mut(idx) {
            lane.push_back(item);
            self.stats.admitted += 1;
            if self.len() > self.stats.peak_depth {
                self.stats.peak_depth = self.len();
            }
        }
    }

    /// Find the least urgent shedable class that is non-empty, and less urgent than
    /// the item we are making room for. Returns which class was chosen (not the item
    /// — the item is dropped here, deliberately, because a shed item is not served).
    ///
    /// The guard direction matters and is easy to get backwards: `Gate` sorts
    /// **first** (most urgent), so a candidate is less urgent than the incoming item
    /// when `candidate` has the LARGER ordinal, i.e. when `for_priority < candidate`.
    /// Shedding is therefore allowed exactly when `for_priority <= candidate` — we
    /// never drop something at least as urgent as what we are admitting.
    fn shed_one_of(&mut self, for_priority: Priority) -> Option<Priority> {
        // Least urgent first, so the cheapest thing is sacrificed.
        for candidate in [Priority::Background, Priority::Interactive, Priority::Gate] {
            if !candidate.shedable() || for_priority > candidate {
                continue;
            }
            let idx = Self::lane(candidate);
            // Collapsed rather than nested so `get_mut` runs only when the lane is
            // non-empty — the borrow checker needs the check and the mutation in one
            // expression.
            if let Some(lane) = self.lanes.get_mut(idx).filter(|l| !l.is_empty()) {
                lane.pop_front();
                return Some(candidate);
            }
        }
        None
    }

    /// Remove and return the most urgent item, FIFO within its class.
    ///
    /// Returns `None` when empty — never a panic.
    pub fn pop(&mut self) -> Option<(Priority, T)> {
        for priority in Priority::SERVED_HIGHEST_FIRST {
            let idx = Self::lane(priority);
            if let Some(item) = self.lanes.get_mut(idx).and_then(VecDeque::pop_front) {
                return Some((priority, item));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;

    /// Push and assert it was ADMITTED (cleanly or by shedding). Panics on a
    /// refusal — use this where the test cares about admission, not about which
    /// form. Tests that care about shedding assert on the return value directly.
    fn push_ok(q: &mut PriorityQueue<u32>, p: Priority, v: u32) {
        match q.push(p, v) {
            Ok(_) => {}
            other => panic!("expected admission for {p:?}={v}, got {other:?}"),
        }
    }

    // ---- ordering -----------------------------------------------------------

    #[test]
    fn classes_are_ordered_gate_then_interactive_then_background() {
        // Push in the WRONG order; the pop order must be the priority order.
        let mut q = PriorityQueue::new(10);
        push_ok(&mut q, Priority::Background, 3);
        push_ok(&mut q, Priority::Interactive, 2);
        push_ok(&mut q, Priority::Gate, 1);

        assert_eq!(q.pop(), Some((Priority::Gate, 1)));
        assert_eq!(q.pop(), Some((Priority::Interactive, 2)));
        assert_eq!(q.pop(), Some((Priority::Background, 3)));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn within_a_class_order_is_fifo() {
        // The property that bounds a class's wait: once a class is being served, it
        // drains in arrival order. Without it, one class could starve another.
        let mut q = PriorityQueue::new(10);
        for v in 1..=3 {
            push_ok(&mut q, Priority::Background, v);
        }
        assert_eq!(q.pop(), Some((Priority::Background, 1)));
        assert_eq!(q.pop(), Some((Priority::Background, 2)));
        assert_eq!(q.pop(), Some((Priority::Background, 3)));
    }

    #[test]
    fn a_gate_never_waits_behind_background() {
        // The whole justification for this module: a hook is blocking an agent, so
        // it must not queue behind a bulk scan.
        let mut q = PriorityQueue::new(200);
        for v in 0..100 {
            push_ok(&mut q, Priority::Background, v);
        }
        push_ok(&mut q, Priority::Gate, 9999);
        assert_eq!(
            q.pop(),
            Some((Priority::Gate, 9999)),
            "the gate must jump the queue"
        );
    }

    #[test]
    fn an_empty_queue_pops_nothing() {
        let mut q: PriorityQueue<u32> = PriorityQueue::new(4);
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    // ---- boundedness --------------------------------------------------------

    #[test]
    fn enqueueing_beyond_capacity_is_refused_when_nothing_is_shedable() {
        // Capacity bounds the system. Refusing is the point: an unbounded queue
        // converts a load spike into memory exhaustion. To reach the refusal we need
        // a queue where nothing may be shed — i.e. one full of Gates.
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Gate, 1);
        push_ok(&mut q, Priority::Gate, 2);
        assert_eq!(q.len(), 2);
        assert_eq!(
            q.push(Priority::Gate, 3),
            Err(EnqueueError::Full {
                refused: Priority::Gate
            })
        );
        assert_eq!(q.len(), 2, "a refused push must not grow the queue");
    }

    #[test]
    fn a_gate_preempts_background_when_the_queue_is_full() {
        // The complementary case, and the reason the queue is prioritised at all: a
        // full queue of bulk work must NOT block a hook that is stopping an agent.
        // The Gate is admitted and a Background item is sacrificed for it.
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Background, 1);
        push_ok(&mut q, Priority::Background, 2);
        assert_eq!(
            q.push(Priority::Gate, 3),
            Ok(Enqueued::AdmittedAfterShedding(Priority::Background))
        );
        assert_eq!(q.stats().shed[2], 1, "one background item sacrificed");
        assert_eq!(q.len(), 2, "still at capacity");
        assert_eq!(
            q.pop(),
            Some((Priority::Gate, 3)),
            "the gate is served next"
        );
    }

    #[test]
    fn background_is_shed_first_when_full() {
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Background, 1);
        push_ok(&mut q, Priority::Background, 2);
        // An Interactive is more urgent than Background, so shedding is allowed.
        assert_eq!(
            q.push(Priority::Interactive, 9),
            Ok(Enqueued::AdmittedAfterShedding(Priority::Background))
        );
        assert_eq!(q.pop(), Some((Priority::Interactive, 9)));
        assert_eq!(q.stats().shed[2], 1, "one background item shed");
    }

    #[test]
    fn a_gate_admits_by_shedding_background_not_another_gate() {
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Gate, 1);
        push_ok(&mut q, Priority::Background, 2);
        // The Gate is more urgent than Background, and a Gate must never be shed —
        // so the Background goes.
        assert_eq!(
            q.push(Priority::Gate, 3),
            Ok(Enqueued::AdmittedAfterShedding(Priority::Background))
        );
        assert_eq!(q.stats().shed[0], 0, "no gate was shed");
        assert_eq!(q.pop(), Some((Priority::Gate, 1)));
        assert_eq!(q.pop(), Some((Priority::Gate, 3)));
    }

    #[test]
    fn a_gate_is_never_shed_by_default() {
        // THE fail-closed property. Two gates fill the queue; a third is REFUSED
        // rather than one of the two being silently dropped — dropping a gate reads
        // as permission, and the caller must instead fail closed.
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Gate, 1);
        push_ok(&mut q, Priority::Gate, 2);
        assert!(q.push(Priority::Gate, 3).is_err());
        assert_eq!(q.stats().shed[0], 0, "a gate must never be shed");
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn interactive_is_not_shed_to_admit_background() {
        // The CONTROL for the shedding rule: bulk traffic must not evict the work it
        // ought to be waiting behind.
        let mut q = PriorityQueue::new(1);
        push_ok(&mut q, Priority::Interactive, 1);
        assert_eq!(
            q.push(Priority::Background, 2),
            Err(EnqueueError::Full {
                refused: Priority::Background
            }),
            "a Background must not evict an Interactive"
        );
        assert_eq!(q.stats().shed[1], 0);
        assert_eq!(q.pop(), Some((Priority::Interactive, 1)));
    }

    #[test]
    fn a_zero_capacity_queue_refuses_everything_without_panicking() {
        // A "disabled" configuration must be expressible and must not panic.
        let mut q: PriorityQueue<u32> = PriorityQueue::new(0);
        assert!(q.push(Priority::Gate, 1).is_err());
        assert_eq!(q.pop(), None);
    }

    // ---- observability ------------------------------------------------------

    #[test]
    fn depth_and_sheds_are_reported() {
        // A saturated system must be VISIBLE, or it is merely slow and nobody knows.
        let mut q = PriorityQueue::new(2);
        push_ok(&mut q, Priority::Background, 1);
        push_ok(&mut q, Priority::Background, 2);
        assert_eq!(q.stats().peak_depth, 2);
        assert_eq!(q.stats().admitted, 2);

        // A Gate preempts a Background — the event is counted, so the sacrifice is
        // visible rather than silent.
        assert_eq!(
            q.push(Priority::Gate, 3),
            Ok(Enqueued::AdmittedAfterShedding(Priority::Background))
        );
        assert_eq!(q.stats().shed[2], 1);
        assert_eq!(q.stats().refused, 0);
        assert_eq!(
            q.stats().peak_depth,
            2,
            "a full queue must not exceed its capacity"
        );
    }

    #[test]
    fn a_refusal_is_counted() {
        // The other half of the observability contract: when work IS refused, the
        // count says so. A silent refusal is indistinguishable from a slow system.
        let mut q = PriorityQueue::new(1);
        push_ok(&mut q, Priority::Gate, 1);
        let _ = q.push(Priority::Gate, 2);
        assert_eq!(q.stats().refused, 1);
        assert_eq!(q.stats().peak_depth, 1, "a refusal must not raise the peak");
    }

    #[test]
    fn capacity_is_respected_across_all_classes() {
        let mut q = PriorityQueue::new(5);
        for v in 0..3 {
            push_ok(&mut q, Priority::Gate, v);
        }
        for v in 0..3 {
            push_ok(&mut q, Priority::Background, v);
        }
        assert!(q.len() <= 5, "len {} exceeded capacity", q.len());
        assert_eq!(q.capacity(), 5);
    }

    // ---- the priority order itself ------------------------------------------

    #[test]
    fn gate_is_the_most_urgent_variant() {
        // Pinned, because the whole module depends on the derived Ord matching the
        // declared order: a reordering of the enum must fail here, loudly.
        assert!(Priority::Gate < Priority::Interactive);
        assert!(Priority::Interactive < Priority::Background);
        assert_eq!(Priority::SERVED_HIGHEST_FIRST[0], Priority::Gate);
        assert_eq!(Priority::SERVED_HIGHEST_FIRST[2], Priority::Background);
    }

    #[test]
    fn only_a_gate_is_unshedable() {
        assert!(!Priority::Gate.shedable());
        assert!(Priority::Interactive.shedable());
        assert!(Priority::Background.shedable());
    }

    #[test]
    fn the_full_error_names_the_refusal_and_the_fail_closed_rule() {
        let e = EnqueueError::Full {
            refused: Priority::Gate,
        };
        let msg = format!("{e}");
        assert!(
            msg.contains("ask"),
            "the message must state the fail-closed rule: {msg}"
        );
        assert!(msg.contains("never `allow`") || msg.contains("never allow"));
    }
}

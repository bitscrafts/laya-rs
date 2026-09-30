//! Print the queue's ordering, so a reader can see the policy rather than trust a description.
//!
//! Spec 06's exit criterion cites this example by name and it **did not exist** — the criterion
//! named a `queue_demo` target the crate never had. That is the "documented capability no caller
//! can reach" class this project keeps finding, arriving through an exit criterion instead of
//! through a doc. So the example is written rather than the criterion deleted: the criterion was
//! asking for something reasonable, and the honest fix is to provide it.
//!
//! Run: `cargo run -q -p laya-core --example queue_demo`

use laya_core::queue::{Enqueued, Priority, PriorityQueue};

fn main() {
    let mut q: PriorityQueue<&str> = PriorityQueue::new(8);

    // Enqueued deliberately out of priority order, so what is printed is the POLICY and not the
    // insertion order. A queue that echoes its input would look correct on a sorted input.
    let pushes: [(Priority, &str); 5] = [
        (Priority::Background, "log_rotation"),
        (Priority::Interactive, "user_question"),
        (Priority::Gate, "gate_before_write"),
        (Priority::Background, "index_rebuild"),
        (Priority::Interactive, "follow_up"),
    ];
    for (p, item) in pushes {
        match q.push(p, item) {
            Ok(Enqueued::Admitted) => {}
            Ok(Enqueued::AdmittedAfterShedding(class)) => {
                println!("  admitted {item} after shedding a {} item", class.as_str());
            }
            Err(e) => println!("  refused {item}: {e}"),
        }
    }

    println!(
        "insertion order : {}",
        pushes.iter().map(|(_, s)| *s).collect::<Vec<_>>().join(" ")
    );
    print!("served order    : ");
    let mut served = Vec::new();
    while let Some((_priority, item)) = q.pop() {
        // The class is implied by the order; the item is what a reader checks against `expect`.
        served.push(item.to_string());
    }
    println!("{}", served.join(" "));
    println!("stats           : {:?}", q.stats());

    // The invariant the criterion is really about: every Gate item precedes every Interactive
    // item, which precedes every Background item.
    let expect = [
        "gate_before_write",
        "user_question",
        "follow_up",
        "log_rotation",
        "index_rebuild",
    ];
    assert_eq!(
        served, expect,
        "served order must be Gate > Interactive > Background"
    );
    println!("OK: Gate > Interactive > Background, insertion order ignored");
}

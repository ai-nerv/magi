use super::*;
use crate::events::collection::enrollment::Host;

mod runner;

pub(super) struct Gate {
    worker: Option<Worker>,
    stalled: Arc<AtomicBool>,
    expired: Arc<AtomicBool>,
    deadline: Instant,
}

impl Gate {
    pub(super) fn new(stalled: bool, budget: Duration) -> Self {
        let (_, collector) = setup();
        let gate = Arc::new(AtomicBool::new(stalled));
        let expired = Arc::new(AtomicBool::new(false));
        let held = gate.clone();
        let timed_out = expired.clone();
        let deadline = Instant::now() + budget;
        let worker = Worker::spawn(
            collector,
            move |mut collector, receiver, stop, published| {
                while held.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                    if Instant::now() >= deadline {
                        timed_out.store(true, Ordering::Release);
                        return collector;
                    }
                    registrations(&mut collector, &receiver);
                    collector.retire(Instant::now());
                    published.store(collector.summary());
                    thread::park_timeout(POLL_INTERVAL);
                }
                run(collector, receiver, stop, published)
            },
        )
        .expect("test observer");
        Self {
            worker: Some(worker),
            stalled: gate,
            expired,
            deadline,
        }
    }

    pub(super) fn worker(&self) -> &Worker {
        self.worker.as_ref().expect("live observer")
    }

    pub(super) fn resume(&self) {
        if self.stalled.swap(false, Ordering::AcqRel) && Instant::now() >= self.deadline {
            self.expired.store(true, Ordering::Release);
        }
        self.worker().registrar.wake.unpark();
    }

    pub(super) fn finish(mut self) -> Summary {
        self.resume();
        let summary = self
            .worker
            .take()
            .expect("observer")
            .shutdown()
            .expect("join");
        assert!(
            !self.expired.load(Ordering::Acquire),
            "observer stall expired"
        );
        summary
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.resume();
        }
    }
}

#[derive(Default)]
pub(super) struct Admissions(Vec<Host>);

impl Admissions {
    pub(super) fn add(&mut self, host: Host) {
        assert!(self.0.len() < MAX_PRODUCERS, "observer host bound");
        self.0.push(host);
    }

    pub(super) fn poll(&mut self) {
        let mut index = 0;
        let mut visited = 0;
        while index < self.0.len() && visited < MAX_PRODUCERS {
            visited += 1;
            let allow = self.0.len() < MAX_PRODUCERS;
            let host = self.0.get_mut(index).expect("host");
            let live = host.poll_branches(allow);
            let branch = host.take_branch();
            if !live {
                self.0.swap_remove(index);
            } else {
                index += 1;
            }
            if let Some(branch) = branch {
                self.add(branch);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::collection::enrollment::Client;

    fn barrier(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(Instant::now() < deadline, "observer barrier");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn native_event(sender: &Sender) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema":1, "program":"casper", "program_version":[1,0,0],
            "producer":sender.identity(), "sequence":1, "correlation":sender.lineage(),
            "fact":{"kind":"tool", "outcome":"succeeded", "duration_ms":1}
        }))
        .expect("supplier event")
    }

    fn nested_admission(stalled: bool) {
        let gate = Gate::new(stalled, Duration::from_secs(10));
        let lineage = Lineage::root(id(2), id(3));
        let (host, mut anchor, socket) =
            Host::pair(gate.worker().registrar(), lineage).expect("parent enrollment");
        let client = Client::adopt(socket, lineage).expect("parent client");
        let mut admissions = Admissions::default();
        admissions.add(host);
        let branch = client.branch(lineage).expect("nested branch");
        let (mut nested, control, _) = branch.into_parts();
        let child = Client::adopt(control, nested.lineage()).expect("nested client");
        barrier(|| {
            admissions.poll();
            child.poll().expect("child control");
            admissions.0.len() == 2
        });
        let mut supplier = child
            .open(Owner::Casper, nested.lineage())
            .expect("supplier");
        barrier(|| {
            admissions.poll();
            gate.worker().readout().published.load().remote_producers == 3
        });
        let bytes = native_event(&supplier.sender);
        assert_eq!(supplier.sender.send(&bytes), Sent::Queued);
        assert_eq!(
            supplier.sender.report(Loss::default(), true),
            Report::Sent(Sent::Queued)
        );
        assert_eq!(
            anchor.report(Loss::default(), true),
            Report::Sent(Sent::Queued)
        );
        assert_eq!(
            nested.report(Loss::default(), true),
            Report::Sent(Sent::Queued)
        );
        if stalled {
            assert_eq!(gate.worker().readout().snapshot().counts.accepted, 0);
            assert_eq!(gate.worker().readout().snapshot().counts.statuses, 0);
        }
        gate.resume();
        wait(&gate.worker().readout(), |s| s.retired.final_confirmed == 3);
        drop(supplier);
        drop(child);
        drop(client);
        barrier(|| {
            admissions.poll();
            admissions.0.is_empty()
        });
        let summary = gate.finish();
        assert_eq!(summary.counts.accepted, 1);
        assert_eq!(summary.counts.invalid, 0);
        assert_eq!(summary.counts.refused_producers, 0);
        assert!(!summary.incomplete());
    }

    #[test]
    fn stalled_data_keeps_nested_and_supplier_admission_responsive() {
        nested_admission(true);
    }

    #[test]
    fn draining_uses_the_same_nested_and_supplier_admission_topology() {
        nested_admission(false);
    }

    #[test]
    fn stalled_retirement_releases_capacity_without_claiming_unread_delivery() {
        let gate = Gate::new(true, Duration::from_secs(10));
        let mut held = Vec::new();
        for _ in 1..MAX_PRODUCERS {
            let mut opened = gate.worker().open(Owner::Casper).expect("producer");
            let bytes = native_event(&opened.sender);
            assert_eq!(opened.sender.send(&bytes), Sent::Queued);
            held.push(opened);
        }
        barrier(|| gate.worker().readout().published.load().remote_producers == MAX_PRODUCERS - 1);
        assert!(matches!(
            gate.worker().open(Owner::Casper),
            Err(RegisterError::Capacity)
        ));
        drop(held);
        wait(&gate.worker().readout(), |s| s.remote_producers == 0);
        assert_eq!(gate.worker().readout().snapshot().counts.accepted, 0);
        let next = gate.worker().open(Owner::Casper).expect("reused capacity");
        drop(next);
        let summary = gate.finish();
        assert!(summary.incomplete());
        assert_eq!(summary.retired.final_confirmed, 0);
        assert!(summary.retired.unknown_loss_producers >= (MAX_PRODUCERS - 1) as u64);
    }

    #[test]
    #[should_panic(expected = "observer stall expired")]
    fn expired_stall_fails_instead_of_silently_draining() {
        let gate = Gate::new(true, Duration::ZERO);
        barrier(|| gate.worker().task.as_ref().expect("task").is_finished());
        assert!(gate.expired.load(Ordering::Acquire));
        assert!(matches!(
            gate.worker().register(Owner::Magi, id(2)),
            Err(RegisterError::Closed)
        ));
        assert_eq!(gate.worker().readout().snapshot().counts.accepted, 0);
        gate.finish();
    }

    #[test]
    #[should_panic(expected = "observer stall expired")]
    fn release_after_deadline_cannot_race_into_a_successful_receipt() {
        let gate = Gate::new(true, Duration::ZERO);
        gate.resume();
        gate.finish();
    }
}

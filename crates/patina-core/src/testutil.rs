//! Helpers for building synthetic logs in unit tests.

use jiff::Timestamp;
use jiff::civil::Date;
use uuid::Uuid;

use crate::event::*;
use crate::ids::*;
use crate::state::{State, project};

pub(crate) struct TestLog {
    pub events: Vec<Event>,
    n: u128,
    pub device: DeviceId,
}

pub(crate) fn date(s: &str) -> Date {
    s.parse().unwrap()
}

impl TestLog {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            n: 100,
            device: DeviceId::from_uuid(Uuid::from_u128(1)),
        }
    }

    pub fn uuid(&mut self) -> Uuid {
        self.n += 1;
        Uuid::from_u128(self.n)
    }

    /// Pushes one event as its own command on `device`.
    pub fn push_on(&mut self, device: DeviceId, data: EventData) -> CommandId {
        self.push_full(device, data, None)
    }

    /// Pushes one event with explicit effective and recorded times.
    pub fn push_at(
        &mut self,
        data: EventData,
        effective_at: Timestamp,
        recorded_at: Timestamp,
    ) -> CommandId {
        let d = self.device;
        self.push_full(d, data, Some((effective_at, recorded_at)))
    }

    fn push_full(
        &mut self,
        device: DeviceId,
        data: EventData,
        times: Option<(Timestamp, Timestamp)>,
    ) -> CommandId {
        let id = self.uuid();
        let command = CommandId::from_uuid(self.uuid());
        let ts = Timestamp::from_second(1_800_000_000 + self.n as i64).unwrap();
        let (effective_at, recorded_at) = times.unwrap_or((ts, ts));
        self.events.push(Event {
            id: EventId::from_uuid(id),
            device,
            effective_at,
            recorded_at,
            source: Source::Cli,
            command,
            category: Category::Domain,
            data,
        });
        command
    }

    pub fn push(&mut self, data: EventData) -> CommandId {
        let d = self.device;
        self.push_on(d, data)
    }

    pub fn identity(&mut self, name: &str) -> IdentityId {
        let id = IdentityId::from_uuid(self.uuid());
        self.push(EventData::IdentityCreated(IdentityCreated {
            identity: id,
            name: name.into(),
        }));
        id
    }

    pub fn item(&mut self, title: &str, identity: IdentityId, parent: Option<ItemId>) -> ItemId {
        let id = ItemId::from_uuid(self.uuid());
        self.push(EventData::ItemCreated(ItemCreated {
            item: id,
            title: title.into(),
            identity,
            parent,
            tags: vec![],
            defer: None,
            scheduled: None,
            deadline: None,
            waiting_for: None,
            follow_up: None,
            important: false,
            estimate: None,
            repeat: None,
            repeat_of: None,
        }));
        id
    }

    pub fn edge(&mut self, item: ItemId, depends_on: ItemId) {
        self.push(EventData::EdgeAdded(EdgeAdded { item, depends_on }));
    }

    pub fn complete(&mut self, item: ItemId) {
        self.push(EventData::ItemCompleted(ItemCompleted { item, note: None }));
    }

    pub fn habit(
        &mut self,
        name: &str,
        identity: IdentityId,
        frequency: Frequency,
        start: &str,
    ) -> HabitId {
        self.habit_of(name, identity, HabitKind::Check, frequency, start)
    }

    pub fn habit_of(
        &mut self,
        name: &str,
        identity: IdentityId,
        kind: HabitKind,
        frequency: Frequency,
        start: &str,
    ) -> HabitId {
        let id = HabitId::from_uuid(self.uuid());
        self.push(EventData::HabitCreated(HabitCreated {
            habit: id,
            name: name.into(),
            identity,
            kind,
            frequency,
            cue: None,
            start: date(start),
        }));
        id
    }

    pub fn check(&mut self, habit: HabitId, day: &str) {
        self.push(EventData::HabitCheckedIn(HabitCheckedIn {
            habit,
            day: date(day),
            count: None,
            note: None,
            backfilled: false,
        }));
    }

    pub fn state(&self) -> State {
        project(&self.events)
    }
}

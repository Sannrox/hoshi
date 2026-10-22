//! Fleet › Ops: register a vehicle and plan a mission with the same core
//! actions as `hoshi vehicle register` and `hoshi mission plan`.

use super::*;
use hoshi_core::{Customer, Payload, PlanRequest, TransferKind};

const TRANSFERS: [TransferKind; 3] = [
    TransferKind::Hohmann,
    TransferKind::HighEnergy,
    TransferKind::Assist,
];
const CARGO: [ResourceKind; 3] = [
    ResourceKind::Iron,
    ResourceKind::Silicon,
    ResourceKind::Deuterium,
];
const FIELDS: usize = 5;

/// The plan form for one vehicle. Fields: destination, customer, transfer,
/// cargo resource, cargo amount.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MissionForm {
    vehicle_id: u64,
    dests: Vec<Coord>,
    dest: usize,
    customer: usize,
    transfer: usize,
    cargo: usize,
    amount: f64,
    field: usize,
}

impl MissionForm {
    fn dest(&self) -> Coord {
        self.dests[self.dest]
    }
}

impl App {
    /// `+` on Ops: park every ship at the selected planet in a new vehicle.
    pub(super) fn register_ops_vehicle(&mut self, now: u64) {
        let planet = self.selected_coord();
        let ships = self.state.parked_fleet(planet).unwrap_or_default();
        self.status = match self.state.register_vehicle(planet, ships, now) {
            Ok(id) => format!("registered vehicle #{id} at {planet}"),
            Err(error) => error.to_string(),
        };
    }

    /// Enter on an Ops vehicle row opens the plan form for it.
    pub(super) fn open_mission_form(&mut self) {
        let Some(vehicle) = self.state.vehicles.get(self.selected) else {
            return;
        };
        let origin = vehicle.coord;
        // Own planets first (colony deliveries), then the map cursor.
        let mut dests: Vec<Coord> = self
            .state
            .owned_coords()
            .into_iter()
            .filter(|&c| c != origin)
            .collect();
        let cursor = self.map_cursor();
        if cursor != origin && !dests.contains(&cursor) {
            dests.push(cursor);
        }
        if dests.is_empty() {
            self.status = "pick a destination on the map first".into();
            return;
        }
        self.mission_form = Some(MissionForm {
            vehicle_id: vehicle.id,
            dests,
            dest: 0,
            customer: 0,
            transfer: 0,
            cargo: 0,
            amount: 0.0,
            field: 0,
        });
    }

    /// Customers that can take a delivery at `dest`: a colony when it is
    /// ours, otherwise a faction.
    fn mission_customers(&self, dest: Coord) -> Vec<Customer> {
        if self.state.owned_coords().contains(&dest) {
            return vec![Customer::Colony];
        }
        self.state
            .config
            .diplomacy
            .iter()
            .map(|spec| Customer::Faction(spec.id.clone()))
            .collect()
    }

    pub(super) fn handle_mission_form(&mut self, code: KeyCode, now: u64) -> Action {
        if self.keymap.matches(Binding::Quit, code) {
            return Action::Quit;
        }
        if self.keymap.matches(Binding::Cancel, code) {
            self.mission_form = None;
            return Action::None;
        }
        if self.keymap.matches(Binding::Confirm, code) {
            self.submit_mission_form(now);
            return Action::None;
        }
        let customers = self
            .mission_form
            .as_ref()
            .map_or(0, |form| self.mission_customers(form.dest()).len());
        let Some(form) = self.mission_form.as_mut() else {
            return Action::None;
        };
        let step = |n: usize, len: usize, by: isize| {
            (n as isize + by).rem_euclid(len.max(1) as isize) as usize
        };
        let by = if self.keymap.matches(Binding::CursorRight, code) {
            1
        } else if self.keymap.matches(Binding::CursorLeft, code) {
            -1
        } else {
            0
        };
        if self.keymap.matches(Binding::SelectNext, code) {
            form.field = (form.field + 1) % FIELDS;
        } else if self.keymap.matches(Binding::SelectPrev, code) {
            form.field = (form.field + FIELDS - 1) % FIELDS;
        } else if self.keymap.matches(Binding::Increase, code) {
            form.amount = (form.amount + 10.0).min(1_000_000.0);
        } else if self.keymap.matches(Binding::Decrease, code) {
            form.amount = (form.amount - 10.0).max(0.0);
        } else if by != 0 {
            match form.field {
                0 => {
                    form.dest = step(form.dest, form.dests.len(), by);
                    form.customer = 0;
                }
                1 => form.customer = step(form.customer, customers, by),
                2 => form.transfer = step(form.transfer, TRANSFERS.len(), by),
                3 => form.cargo = step(form.cargo, CARGO.len(), by),
                _ => form.amount = (form.amount + 10.0 * by as f64).clamp(0.0, 1_000_000.0),
            }
        }
        Action::None
    }

    fn submit_mission_form(&mut self, now: u64) {
        let Some(form) = self.mission_form.clone() else {
            return;
        };
        let Some(customer) = self
            .mission_customers(form.dest())
            .get(form.customer)
            .cloned()
        else {
            self.status = "no customer for that destination".into();
            return;
        };
        let mut cargo = Resources::ZERO;
        cargo.set(CARGO[form.cargo], form.amount);
        let request = PlanRequest {
            vehicle_id: form.vehicle_id,
            dest: form.dest(),
            transfer: TRANSFERS[form.transfer],
            customer,
            payload: Payload { cargo },
            window_index: None,
        };
        match self.state.plan_mission(request, now) {
            Ok(id) => {
                self.mission_form = None;
                self.status = format!("planned mission {id}");
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    pub(super) fn draw_mission_form(&self, frame: &mut Frame<'_>) {
        let Some(form) = &self.mission_form else {
            return;
        };
        let t = &self.theme;
        let customer = self
            .mission_customers(form.dest())
            .get(form.customer)
            .map_or_else(|| "none".to_string(), Customer::as_key);
        let rows = [
            ("destination", form.dest().to_string()),
            ("customer", customer),
            (
                "transfer",
                TRANSFERS[form.transfer].as_key().replace('_', " "),
            ),
            ("cargo", CARGO[form.cargo].label().to_string()),
            ("amount", format!("{:.0}", form.amount)),
        ];
        let mut lines: Vec<Line<'static>> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (label, value))| {
                let marker = if i == form.field { "▸ " } else { "  " };
                Line::from(vec![
                    Span::styled(format!("{marker}{label:<12}"), t.label()),
                    Span::styled(format!("‹ {value} ›"), t.text()),
                ])
            })
            .collect();
        if !self.status.is_empty() {
            lines.push(Line::default());
            lines.push(Line::styled(self.status.clone(), t.muted()));
        }
        self.dialog(
            frame,
            "Plan mission",
            format!("vehicle #{}", form.vehicle_id),
            lines,
            &[
                (self.keymap.primary(Binding::SelectNext), "field"),
                (self.keymap.primary(Binding::CursorRight), "change"),
                (self.keymap.primary(Binding::Increase), "amount"),
                (self.keymap.primary(Binding::Confirm), "plan"),
                (self.keymap.primary(Binding::Cancel), "cancel"),
            ],
        );
    }
}

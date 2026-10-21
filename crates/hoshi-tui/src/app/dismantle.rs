use super::*;

impl App {
    pub(super) fn handle_dismantle_confirm(&mut self, code: KeyCode, now: u64) -> Action {
        if self.keymap.matches(Binding::Quit, code) {
            return Action::Quit;
        }
        if self.keymap.matches(Binding::Cancel, code) {
            self.dismantle_confirm = None;
            return Action::None;
        }
        if self.keymap.matches(Binding::Confirm, code)
            && let Some((planet, building)) = self.dismantle_confirm
        {
            let intent = hoshi_core::Intent::Dismantle {
                planet,
                building,
                actor: Actor::Player,
            };
            match hoshi_core::propose(&self.state, &intent, now)
                .and_then(|proposal| hoshi_core::apply(&mut self.state, &proposal, now))
            {
                Ok(()) => {
                    self.dismantle_confirm = None;
                    self.status = "queued dismantle".into();
                }
                Err(error) => self.status = error.to_string(),
            }
        }
        Action::None
    }

    pub(super) fn draw_dismantle_confirm(&self, frame: &mut Frame<'_>) {
        let Some((planet, building)) = self.dismantle_confirm else {
            return;
        };
        let Ok(preview) = self.state.dismantle_preview(planet, building) else {
            return;
        };
        let mut lines = vec![
            Line::from(format!(
                "{} {} → {}",
                building.as_key().replace('_', " "),
                preview.from_level,
                preview.from_level.saturating_sub(1)
            )),
            Line::default(),
            Line::from(format!(
                "Time                     {}",
                ui::spans::duration(preview.duration_secs)
            )),
            Line::from(format!(
                "Power during work        {:.1} /s",
                preview.energy_draw
            )),
            Line::from(format!(
                "Power freed after work   {:.1} /s",
                preview.energy_freed
            )),
            Line::from(format!(
                "Heat freed after work    {:.1} /s",
                preview.heat_freed
            )),
            Line::default(),
            Line::from(format!("Salvage  {}", preview.salvage.brief())),
            Line::from(format!("Condition  {:.0}%", preview.condition * 100.0)),
            Line::from("Production stops during work."),
            Line::from("Salvage enters normal stock; overflow is lost."),
        ];
        for blocker in &preview.blockers {
            lines.push(Line::from(format!("Blocked: {blocker}")));
        }
        let hints = if preview.blockers.is_empty() {
            vec![
                (self.keymap.primary(Binding::Confirm), "queue dismantle"),
                (self.keymap.primary(Binding::Cancel), "keep building"),
            ]
        } else {
            vec![(self.keymap.primary(Binding::Cancel), "back")]
        };
        self.dialog(
            frame,
            if preview.blockers.is_empty() {
                "Dismantle one level?"
            } else {
                "Cannot dismantle"
            },
            planet.to_string(),
            lines,
            &hints,
        );
    }
}

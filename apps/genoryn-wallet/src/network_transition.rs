//! Applying a network privacy mode to the node this wallet owns.
//!
//! Exactly one transition runs at a time and each step waits until the previous
//! process operation has finished, so repeated clicks never start a second node.
//! Status reports from before a change carry an older generation and are
//! discarded. A transition never falls back to a direct connection: if Tor does
//! not come up, the node keeps waiting for Tor in the selected mode.
use super::*;
use network_privacy::Mode;

/// How long the restarted node may take to answer with the selected mode. Tor
/// bootstrap itself may take longer; it is shown as progress, not waited for.
pub(super) const NODE_REPORT_PATIENCE: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    StoppingMiner,
    StoppingNode,
    StartingNode,
    WaitingForNode { since: Instant },
    ResumingMiner,
}
impl Phase {
    pub fn describe(self) -> &'static str {
        match self {
            Self::StoppingMiner => "Pausing the miner",
            Self::StoppingNode => "Stopping the node",
            Self::StartingNode => "Starting the node",
            Self::WaitingForNode { .. } => "Waiting for the node to report the new mode",
            Self::ResumingMiner => "Resuming the miner",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Transition {
    pub target: Mode,
    pub phase: Phase,
    /// The miner was running when the change began and is restarted afterwards,
    /// paying the same wallet it paid before.
    pub resume_miner: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Applied(Mode),
    Failed(String),
    Cancelled,
}

impl WalletApp {
    /// Applies `target` to the owned node, or re-targets a change in progress.
    /// Without an owned node the choice is only saved for the next start.
    pub(super) fn apply_network_mode(&mut self, target: Mode) {
        self.network_generation = self.network_generation.wrapping_add(1);
        self.network_privacy = None;
        self.network_outcome = None;
        self.last_health = Instant::now() - Duration::from_secs(10);
        match &mut self.network_transition {
            Some(transition) => {
                transition.target = target;
                // A node already started with the previous arguments is restarted.
                if matches!(
                    transition.phase,
                    Phase::WaitingForNode { .. } | Phase::ResumingMiner
                ) {
                    transition.phase = Phase::StoppingNode;
                }
            }
            None if self.node.is_some() => {
                let resume_miner = self.miner.is_some();
                self.miner_retry_at = None;
                self.network_transition = Some(Transition {
                    target,
                    phase: if resume_miner {
                        Phase::StoppingMiner
                    } else {
                        Phase::StoppingNode
                    },
                    resume_miner,
                });
            }
            None => {}
        }
    }
    /// Stops driving the change. Processes keep whatever state they reached; a
    /// stopped node stays stopped until the user starts it.
    pub(super) fn cancel_network_transition(&mut self) {
        if self.network_transition.take().is_some() {
            self.network_generation = self.network_generation.wrapping_add(1);
            self.network_outcome = Some(Outcome::Cancelled);
            self.status = "Network change cancelled. Start the node again from the Node page if it is stopped.".into();
        }
    }
    fn finish_transition(&mut self, outcome: Outcome) {
        self.status = match &outcome {
            Outcome::Applied(Mode::Direct) => "Network mode applied: Direct.".into(),
            Outcome::Applied(mode) => format!(
                "Network mode applied: {}. Tor keeps connecting in the background; nothing is sent directly meanwhile.",
                mode.label()
            ),
            Outcome::Failed(reason) => format!("Network change not completed: {reason}"),
            Outcome::Cancelled => "Network change cancelled.".into(),
        };
        self.network_outcome = Some(outcome);
    }
    /// Advances the change by at most one step; called once per frame.
    pub(super) fn network_transition_tick(&mut self) {
        if self.process_busy {
            return;
        }
        let Some(mut transition) = self.network_transition.take() else {
            return;
        };
        let mut done = None;
        match transition.phase {
            Phase::StoppingMiner => {
                if self.miner.is_some() {
                    self.stop_process("miner");
                } else {
                    transition.phase = Phase::StoppingNode;
                }
            }
            Phase::StoppingNode => {
                if self.node.is_some() {
                    self.stop_process("node");
                } else {
                    transition.phase = Phase::StartingNode;
                }
            }
            Phase::StartingNode => {
                self.start_process("node");
                if self.process_busy {
                    transition.phase = Phase::WaitingForNode {
                        since: Instant::now(),
                    };
                } else {
                    done = Some(Outcome::Failed(self.status.clone()));
                }
            }
            Phase::WaitingForNode { since } => {
                let reported = self
                    .network_privacy
                    .as_ref()
                    .and_then(|status| status["mode"].as_str())
                    .and_then(Mode::parse);
                if self.node.is_none() {
                    done = Some(Outcome::Failed(
                        "the node exited while starting; open its log on the Node page".into(),
                    ));
                } else if reported == Some(transition.target) {
                    if transition.resume_miner {
                        transition.phase = Phase::ResumingMiner;
                    } else {
                        done = Some(Outcome::Applied(transition.target));
                    }
                } else if since.elapsed() > NODE_REPORT_PATIENCE {
                    done = Some(Outcome::Failed(format!(
                        "the node did not report {} within {} seconds",
                        transition.target.label(),
                        NODE_REPORT_PATIENCE.as_secs()
                    )));
                }
            }
            Phase::ResumingMiner => {
                if self.miner.is_some() {
                    done = Some(Outcome::Applied(transition.target));
                } else if self.node_info.is_some() && self.miner_destination.is_some() {
                    self.start_process("miner");
                    done = Some(Outcome::Applied(transition.target));
                } else if self.node.is_none() {
                    done = Some(Outcome::Failed(
                        "the node stopped before the miner could resume".into(),
                    ));
                }
            }
        }
        match done {
            Some(outcome) => self.finish_transition(outcome),
            None => self.network_transition = Some(transition),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> WalletApp {
        let ctx = egui::Context::default();
        let settings = Settings {
            bin_dir: std::env::temp_dir()
                .join("genoryn-no-binaries-here")
                .display()
                .to_string(),
            ..Settings::default()
        };
        WalletApp::with_context(&ctx, settings)
    }

    #[test]
    fn without_an_owned_node_the_mode_is_only_saved() {
        let mut app = app();
        let before = app.network_generation;
        app.apply_network_mode(Mode::Direct);
        assert!(app.network_transition.is_none());
        assert_ne!(app.network_generation, before, "older reports are discarded");
    }

    #[test]
    fn repeated_choices_retarget_the_single_transition() {
        let mut app = app();
        app.network_transition = Some(Transition {
            target: Mode::TorOnly,
            phase: Phase::WaitingForNode {
                since: Instant::now(),
            },
            resume_miner: false,
        });
        app.apply_network_mode(Mode::Direct);
        app.apply_network_mode(Mode::Hybrid);
        let transition = app.network_transition.unwrap();
        assert_eq!(transition.target, Mode::Hybrid);
        assert_eq!(
            transition.phase,
            Phase::StoppingNode,
            "a node started with the old arguments is restarted once more"
        );
    }

    #[test]
    fn transition_waits_while_a_process_operation_runs() {
        let mut app = app();
        app.process_busy = true;
        app.network_transition = Some(Transition {
            target: Mode::Direct,
            phase: Phase::StoppingNode,
            resume_miner: false,
        });
        app.network_transition_tick();
        assert_eq!(app.network_transition.unwrap().phase, Phase::StoppingNode);
    }

    #[test]
    fn a_node_that_cannot_start_ends_the_transition_with_its_reason() {
        let mut app = app();
        app.network_transition = Some(Transition {
            target: Mode::Direct,
            phase: Phase::StoppingNode,
            resume_miner: false,
        });
        app.network_transition_tick();
        assert_eq!(app.network_transition.unwrap().phase, Phase::StartingNode);
        app.network_transition_tick();
        if let Some(worker) = app.process_worker.take() {
            let _ = worker.join();
        }
        app.poll();
        // The start either failed synchronously or the worker reported the error.
        while app.network_transition.is_some() {
            app.network_transition_tick();
            if app.process_busy {
                if let Some(worker) = app.process_worker.take() {
                    let _ = worker.join();
                }
                app.poll();
            }
        }
        assert!(matches!(app.network_outcome, Some(Outcome::Failed(_))));
    }

    #[test]
    fn only_a_report_of_the_target_mode_completes_the_change() {
        let mut app = app();
        app.network_transition = Some(Transition {
            target: Mode::TorOnly,
            phase: Phase::WaitingForNode {
                since: Instant::now(),
            },
            resume_miner: false,
        });
        // No owned node: the wait ends as a failure, never as success.
        app.network_privacy = Some(json!({"mode": "tor-only"}));
        app.network_transition_tick();
        assert!(matches!(app.network_outcome, Some(Outcome::Failed(_))));
    }

    #[test]
    fn stale_health_reports_cannot_overwrite_the_new_state() {
        let mut app = app();
        let old = app.network_generation;
        app.apply_network_mode(Mode::TorOnly);
        app.health_busy = true;
        app.tx
            .send(Event::Health {
                generation: old,
                endpoint: app.settings.rpc_url.clone(),
                result: Box::new(Err("old node".into())),
                privacy: Some(json!({"mode": "direct"})),
            })
            .unwrap();
        app.poll();
        assert!(!app.health_busy);
        assert!(app.network_privacy.is_none(), "stale report discarded");
        assert!(app.network_error.is_none());
    }

    #[test]
    fn cancel_stops_driving_and_invalidates_reports() {
        let mut app = app();
        app.network_transition = Some(Transition {
            target: Mode::Hybrid,
            phase: Phase::StoppingNode,
            resume_miner: true,
        });
        let before = app.network_generation;
        app.cancel_network_transition();
        assert!(app.network_transition.is_none());
        assert_eq!(app.network_outcome, Some(Outcome::Cancelled));
        assert_ne!(app.network_generation, before);
    }
}

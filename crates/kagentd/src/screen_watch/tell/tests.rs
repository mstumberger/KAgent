use std::sync::Mutex;

use super::*;
use crate::screen_watch::{Mode, State, Target};

#[derive(Default)]
struct Fake {
    messages: Mutex<Vec<(String, Message)>>,
    said: Mutex<Vec<String>>,
    notes: Mutex<Vec<Notification>>,
    live: bool,
    telegram: bool,
}

#[async_trait]
impl Deliver for Fake {
    async fn session_message(&self, session_id: &str, message: Message) -> Result<(), String> {
        self.messages.lock().unwrap().push((session_id.into(), message));
        Ok(())
    }
    fn say(&self, _session_id: &str, text: &str) -> bool {
        if self.live {
            self.said.lock().unwrap().push(text.into());
        }
        self.live
    }
    async fn notify(&self, notification: Notification) {
        self.notes.lock().unwrap().push(notification);
    }
    fn telegram_on(&self) -> bool {
        self.telegram
    }
}

fn watch() -> Watch {
    Watch {
        id: "w_1a2b".into(),
        session_id: "s9".into(),
        goal: "the build finishes".into(),
        target: Target::Window,
        window_id: None,
        window_title: None,
        until: String::new(),
        minutes: 20,
        every_sec: 10,
        mode: Mode::Once,
        state: State::Met,
        looks: 3,
        last_look_at: None,
        last_note: String::new(),
        paused: false,
        reason: None,
    }
}

const HIT: &str = "Screen watch: build finished, no errors. (goal: the build finishes)";

#[tokio::test]
async fn a_hit_is_a_session_message_marked_watch_and_a_desktop_notification_when_nobody_listens() {
    let d = Fake::default();
    tell(&d, &watch(), HIT).await;
    let m = d.messages.lock().unwrap();
    assert_eq!(m.len(), 1);
    let (session, msg) = &m[0];
    assert_eq!(session, "s9");
    assert_eq!((msg.role, msg.via, msg.content.as_str()), (kagent_core::protocol::Role::Assistant, Via::Watch, HIT));
    assert_eq!(serde_json::to_value(msg).unwrap()["via"], "watch");
    let n = d.notes.lock().unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!((n[0].channels.clone(), n[0].title.as_str(), n[0].body.as_str()), (vec![Channel::Desktop], "Screen watch", HIT));
    assert!(d.said.lock().unwrap().is_empty());
}

#[tokio::test]
async fn with_a_live_session_open_it_is_spoken_and_no_notification_is_shown() {
    let d = Fake { live: true, ..Default::default() };
    tell(&d, &watch(), HIT).await;
    assert_eq!(*d.said.lock().unwrap(), vec!["Screen watch: build finished, no errors."], "spoken without the goal tail");
    assert!(d.notes.lock().unwrap().is_empty(), "the voice said it: no second notice");
    assert_eq!(d.messages.lock().unwrap().len(), 1, "and the session still has the message");
}

#[tokio::test]
async fn telegram_gets_it_when_switched_on_with_or_without_the_voice() {
    for live in [false, true] {
        let d = Fake { live, telegram: true, ..Default::default() };
        tell(&d, &watch(), HIT).await;
        let n = d.notes.lock().unwrap();
        assert_eq!(n.len(), 1, "live {live}");
        assert_eq!(n[0].channels.contains(&Channel::Telegram), true);
        assert_eq!(n[0].channels.contains(&Channel::Desktop), !live);
    }
}

#[tokio::test]
async fn the_end_of_the_time_goes_by_the_same_channels() {
    let d = Fake::default();
    tell(&d, &watch(), "Screen watch ended after 20 minutes: still compiling").await;
    assert_eq!(d.notes.lock().unwrap()[0].body, "Screen watch ended after 20 minutes: still compiling");
    assert_eq!(d.messages.lock().unwrap()[0].1.via, Via::Watch);
}

#[tokio::test]
async fn a_restart_note_is_only_a_session_message() {
    let d = Fake { live: true, telegram: true, ..Default::default() };
    restarted(&d, &watch(), "The screen watch ended with the restart. (goal: the build finishes)").await;
    assert_eq!(d.messages.lock().unwrap().len(), 1);
    assert!(d.notes.lock().unwrap().is_empty() && d.said.lock().unwrap().is_empty());
}

#[test]
fn what_is_spoken_has_no_goal_tail() {
    assert_eq!(spoken("Screen watch ended after 1 minute: fine"), "Screen watch ended after 1 minute: fine");
    assert_eq!(spoken("Screen watch: done. (goal: a (b) c)"), "Screen watch: done.");
}

//! Agent sessions across computers: offering this computer's agent tabs to
//! paired devices, and following a paired host's agent tab here.
//!
//! The host keeps running the session. A view on another computer holds a
//! replica controller that follows the host's published view and sends
//! commands back, so the pane renders and reacts the same way for both.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Context, Entity, Task, WeakEntity, Window};
use nmt_agent::chat::Item;
use nmt_agent::session::AgentKind;
use nmt_agent::session::command::{AgentCommand, PromptImage, run_remote};
use nmt_agent::session::controller::{SessionController, SessionEffect};
use nmt_agent::session::input::{QuestionCompletion, QuestionKey};
use nmt_agent::session::view::{
    AgentView, IMAGE_METHOD, ImageData, ImageRef, ViewOp, ViewPublisher, ViewSlot,
};
use nmt_agent::transcript::conversation::ConversationImage;
use nmt_config::profile::AgentProfile;
use nmt_platform::runtime;
use nmt_remote::connection::{AgentLink, AgentUpdate, RemoteHost};
use nmt_remote::sessions::AgentRequest;
use rust_i18n::t;
use serde_json::Value;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

use crate::agent_tab::composer::attachments::scratch_dir;
use crate::agent_tab::execution::{AgentSession, SessionOwner};
use crate::agent_tab::view::recent_sessions::RecentSessionsMode;
use crate::agent_tab::{AgentPane, AgentPaneEvent};

/// How often attached views hear about changes. Streamed text changes far
/// more often; batching keeps a busy turn to a few messages a second, and a
/// command's own result goes out at once.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

/// Answer paired devices' requests for one host agent tab until the tab
/// closes or the registry lets go of it.
pub fn serve(
    session: WeakEntity<AgentSession>,
    mut requests: UnboundedReceiver<AgentRequest>,
    cx: &mut App,
) -> Task<()> {
    cx.spawn(async move |cx| {
        let mut publisher = ViewPublisher::default();
        let mut views: Vec<UnboundedSender<Value>> = Vec::new();

        loop {
            // With nobody watching there is nothing to publish, so the task
            // sleeps until a device asks for something.
            let first = if views.is_empty() {
                match requests.recv().await {
                    Some(request) => Some(request),
                    None => break,
                }
            } else {
                cx.background_executor().timer(PUBLISH_INTERVAL).await;

                match requests.try_recv() {
                    Ok(request) => Some(request),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            };

            let served = session.update(cx, |session, cx| {
                if let Some(request) = first {
                    answer(session, request, &mut publisher, &mut views, cx);
                }

                while let Ok(request) = requests.try_recv() {
                    answer(session, request, &mut publisher, &mut views, cx);
                }

                publish(session, &mut publisher, &mut views);
            });

            if served.is_err() {
                break;
            }
        }
    })
}

fn answer(
    session: &mut AgentSession,
    request: AgentRequest,
    publisher: &mut ViewPublisher,
    views: &mut Vec<UnboundedSender<Value>>,
    cx: &mut Context<AgentSession>,
) {
    match request {
        AgentRequest::Attach { updates, reply } => {
            // Views already attached catch up first, so the snapshot and
            // the changes after it share one baseline.
            publish(session, publisher, views);

            let view = publisher.snapshot(&session.controller.borrow());

            match serde_json::to_value(view) {
                Ok(view) => {
                    if reply.send(view).is_ok() {
                        views.push(updates);
                    }
                }
                Err(error) => warn!(%error, "cannot encode an agent view"),
            }
        }
        AgentRequest::Call {
            method,
            params,
            reply,
        } if method == IMAGE_METHOD => {
            let _ = reply.send(image(session, params));
        }
        AgentRequest::Call {
            method,
            params,
            reply,
        } => {
            let route = session.route.as_str().to_owned();

            let outcome = run_remote(
                &method,
                params,
                &mut session.controller.borrow_mut(),
                |images| stage_images(&route, images),
            )
            .map_err(|error| error.to_string());

            // The host's own pane shows what the command changed.
            session.publish(SessionEffect::Changed, cx);

            publish(session, publisher, views);

            let _ = reply.send(outcome);
        }
    }
}

fn publish(
    session: &AgentSession,
    publisher: &mut ViewPublisher,
    views: &mut Vec<UnboundedSender<Value>>,
) {
    if views.is_empty() {
        return;
    }

    let ops = publisher.changes(&session.controller.borrow());

    if ops.is_empty() {
        return;
    }

    match serde_json::to_value(ops) {
        Ok(ops) => views.retain(|view| view.send(ops.clone()).is_ok()),
        Err(error) => warn!(%error, "cannot encode agent view changes"),
    }
}

fn image(session: &AgentSession, params: Value) -> Result<Value, String> {
    let ImageRef { id, .. } = serde_json::from_value(params).map_err(|error| error.to_string())?;

    let image = session
        .controller
        .borrow()
        .conversation_image(id)
        .ok_or_else(|| format!("no image {id}"))?;

    serde_json::to_value(ImageData {
        bytes: image.bytes.clone(),
    })
    .map_err(|error| error.to_string())
}

/// Write a remote prompt's images where this session's composer keeps its
/// own, for a harness that reads images by path. A failed write leaves the
/// prompt short of paths, which such a harness refuses rather than sending
/// a message without its images.
fn stage_images(route: &str, images: &[PromptImage]) -> Vec<PathBuf> {
    let dir = scratch_dir(route);

    if let Err(error) = fs::create_dir_all(&dir) {
        warn!(%error, "cannot stage remote images");

        return Vec::new();
    }

    images
        .iter()
        .map_while(|image| {
            let extension = image
                .media_type
                .rsplit('/')
                .next()
                .filter(|extension| extension.chars().all(char::is_alphanumeric))
                .unwrap_or("png");

            let path = dir.join(format!("remote-{}.{extension}", Uuid::new_v4()));

            fs::write(&path, &image.bytes)
                .inspect_err(|error| warn!(%error, "cannot stage a remote image"))
                .ok()
                .map(|()| path)
        })
        .collect()
}

/// A pane's link to the host session it follows.
pub struct RemoteAgent {
    host: Arc<RemoteHost>,
    link: Arc<AgentLink>,
    _replica: Task<()>,
}

impl RemoteAgent {
    /// The host and session, as a saved tab records them.
    pub fn address(&self) -> (String, String) {
        (
            self.host.id().as_str().to_owned(),
            self.link.session().to_owned(),
        )
    }

    /// Send `command` to the host and return its outcome.
    pub(crate) fn send<C: AgentCommand>(&self, command: &C) -> JoinHandle<Result<C::Outcome>> {
        let link = Arc::clone(&self.link);
        let params = serde_json::to_value(command);

        runtime().spawn(async move {
            let value = link.call(C::METHOD, params?).await?;

            Ok(serde_json::from_value(value)?)
        })
    }
}

/// Open a view of `session`, an agent tab on `host`, as the owner and pane
/// of a tab.
pub fn open(
    host: Arc<RemoteHost>,
    session: String,
    kind: AgentKind,
    window: &mut Window,
    cx: &mut App,
) -> (SessionOwner, Entity<AgentPane>) {
    let profile = AgentProfile {
        name: host.name(),
        kind,
        ..AgentProfile::default()
    };

    let owner = AgentSession::create(profile, Default::default(), None, cx);

    owner
        .session()
        .update(cx, |session, _| session.follow_remote());

    let (link, updates) = host.agent_view(session);
    let link = Arc::new(link);

    let replica = follow(owner.session().downgrade(), Arc::clone(&link), updates, cx);

    let remote = RemoteAgent {
        host,
        link,
        _replica: replica,
    };

    let pane = cx.new(|cx| {
        let mut pane = AgentPane::attach(&owner, window, cx);

        pane.remote = Some(remote);

        // The recent-sessions list reads this computer's conversation
        // history, which says nothing about the host's.
        pane.history_ui.mode = RecentSessionsMode::Hidden;

        pane.refresh_git_branch(cx);

        pane
    });

    (owner, pane)
}

/// Keep `session`'s replica controller in step with the host.
fn follow(
    session: WeakEntity<AgentSession>,
    link: Arc<AgentLink>,
    mut updates: UnboundedReceiver<AgentUpdate>,
    cx: &mut App,
) -> Task<()> {
    cx.spawn(async move |cx| {
        let mut images: HashMap<Uuid, Arc<ConversationImage>> = HashMap::new();

        while let Some(update) = updates.recv().await {
            let ops = match update {
                AgentUpdate::Snapshot(view) => {
                    serde_json::from_value::<AgentView>(view).map(AgentView::into_ops)
                }
                AgentUpdate::Ops(ops) => serde_json::from_value::<Vec<ViewOp>>(ops),
                AgentUpdate::Ended => {
                    let _ = session.update(cx, |session, cx| {
                        session.controller.borrow_mut().push_item(Item::Error {
                            text: t!("agent-remote-ended").into_owned(),
                        });

                        session.publish(SessionEffect::Changed, cx);
                    });

                    break;
                }
            };

            let ops = match ops {
                Ok(ops) => ops,
                Err(error) => {
                    warn!(%error, "ignoring an undecodable agent view update");

                    continue;
                }
            };

            fetch_images(&link, &ops, &mut images).await;

            let applied = session.update(cx, |session, cx| {
                let applied = apply(session, ops, &images);

                for effect in applied.effects {
                    session.publish(effect, cx);
                }

                if let Some(title) = applied.title {
                    cx.emit(AgentPaneEvent::TitleSuggested(title));
                }
            });

            if applied.is_err() {
                break;
            }
        }
    })
}

/// What the pane has to hear after the replica changed: the effects a
/// session running here would have raised for the same change.
struct Applied {
    effects: Vec<SessionEffect>,
    title: Option<String>,
}

/// The parts of the replica whose transitions the pane reacts to.
struct Presented {
    working: bool,
    pending: Vec<(usize, QuestionKey)>,
    title: Option<String>,
}

impl Presented {
    fn of(controller: &SessionController) -> Self {
        Self {
            working: controller.conversation().borrow().live.is_working(),
            pending: controller
                .input()
                .batches()
                .iter()
                .enumerate()
                .filter(|(_, draft)| draft.pending())
                .map(|(index, draft)| (index, draft.key()))
                .collect(),
            title: controller.conversation_title().map(str::to_owned),
        }
    }
}

/// Apply operations to the replica and work out what the pane has to hear.
fn apply(
    session: &AgentSession,
    ops: Vec<ViewOp>,
    images: &HashMap<Uuid, Arc<ConversationImage>>,
) -> Applied {
    let mut controller = session.controller.borrow_mut();

    let before = Presented::of(&controller);

    let mut catalogs = false;

    for op in ops {
        match op {
            ViewOp::Splice { from, entries } => {
                let entries = entries
                    .into_iter()
                    .map(|entry| {
                        let attached = entry
                            .images
                            .iter()
                            .filter_map(|image| images.get(&image.id).cloned())
                            .collect();

                        entry.into_entry(attached)
                    })
                    .collect();

                controller.splice_transcript(from, entries);
            }
            ViewOp::Slot { slot } => {
                catalogs |= matches!(*slot, ViewSlot::Catalogs(_));

                controller.apply_slot(*slot);
            }
        }
    }

    let after = Presented::of(&controller);

    let mut effects = vec![SessionEffect::Changed];

    // The composer caches the catalogs it offers.
    if catalogs {
        effects.push(SessionEffect::Commands);
        effects.push(SessionEffect::Skills);
    }

    match (before.working, after.working) {
        (false, true) => effects.push(SessionEffect::TurnStarted { opened: true }),
        (true, false) => effects.push(SessionEffect::TurnCompleted { error: None }),
        _ => {}
    }

    effects.extend(
        after
            .pending
            .iter()
            .filter(|asked| !before.pending.iter().any(|seen| seen.1 == asked.1))
            .map(|(index, _)| SessionEffect::InputRequested { index: *index }),
    );

    if before
        .pending
        .iter()
        .any(|seen| !after.pending.iter().any(|asked| asked.1 == seen.1))
    {
        effects.push(SessionEffect::InputResolved(QuestionCompletion {
            message: None,
            started_turn: false,
            waiting_finished: false,
        }));
    }

    Applied {
        effects,
        title: after
            .title
            .filter(|title| before.title.as_ref() != Some(title)),
    }
}

/// Fetch the images `ops` show that this view does not hold yet. An image
/// that cannot be fetched is left out of its entry.
async fn fetch_images(
    link: &Arc<AgentLink>,
    ops: &[ViewOp],
    images: &mut HashMap<Uuid, Arc<ConversationImage>>,
) {
    let wanted: Vec<ImageRef> = ops
        .iter()
        .filter_map(|op| match op {
            ViewOp::Splice { entries, .. } => Some(entries),
            ViewOp::Slot { .. } => None,
        })
        .flatten()
        .flat_map(|entry| entry.images.iter().copied())
        .filter(|image| !images.contains_key(&image.id))
        .collect();

    for image in wanted {
        let link = Arc::clone(link);

        let fetched = runtime()
            .spawn(async move {
                let value = link
                    .call(IMAGE_METHOD, serde_json::to_value(image)?)
                    .await?;

                anyhow::Ok(serde_json::from_value::<ImageData>(value)?)
            })
            .await
            .context("fetching stopped")
            .and_then(|fetched| fetched);

        match fetched {
            Ok(ImageData { bytes }) => {
                images.insert(
                    image.id,
                    Arc::new(ConversationImage {
                        id: image.id,
                        bytes,
                    }),
                );
            }
            Err(error) => warn!(%error, "cannot fetch a transcript image"),
        }
    }
}

//! The property panel beside an editable canvas: the selected node, the
//! definition's slots and its settings.
//!
//! A text field applies its value as one edit when it loses focus or Enter
//! is pressed, so typing a name does not add an undo step per key. Each
//! field remembers the value it was last filled with and is refilled only
//! when the definition's value moves away from that, so a reload or an undo
//! updates it while text being typed stays.

#[cfg(test)]
#[path = "properties_tests.rs"]
mod properties_tests;

use std::iter;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, SharedString, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use nmt_agent::agent_spec::ProfileReference;
use nmt_agent::chat::ThreadSettings;
use nmt_agent::orchestration::definition::{Definition, SlotBody};
use nmt_agent::orchestration::edit::Edit;
use nmt_agent::orchestration::graph::DefinitionError;
use rust_i18n::t;

use crate::agent_tab::orchestration::editor::DefinitionEditor;
use crate::agent_tab::settings::{AgentSettings, UI_RADIUS};

pub(super) const PANEL_WIDTH: f32 = 280.;

/// The thread settings a slot can set, by their key in the file.
const SETTING_KEYS: [&str; 7] = [
    "model",
    "approval",
    "approvals_reviewer",
    "sandbox",
    "effort",
    "tier",
    "agent_preset",
];

/// A text field whose value becomes an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    NodeId,
    NodePrompt,
    SlotName,
    SlotRole,
    Setting(usize),
    MaxParallel,
}

impl Field {
    /// Every field, renames last: the other fields find their node and slot
    /// by the name from before a rename.
    pub(super) fn in_commit_order() -> impl Iterator<Item = Self> {
        [Self::NodePrompt, Self::SlotRole, Self::MaxParallel]
            .into_iter()
            .chain((0..SETTING_KEYS.len()).map(Self::Setting))
            .chain([Self::NodeId, Self::SlotName])
    }
}

/// A picked value that becomes an edit.
pub(super) enum Choice {
    NodeSlot(String),
    SlotProfile(ProfileReference),
}

#[derive(Clone, Debug, PartialEq)]
struct ProfileItem {
    reference: ProfileReference,
    title: SharedString,
}

impl SearchableListItem for ProfileItem {
    type Value = ProfileReference;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &ProfileReference {
        &self.reference
    }
}

type SlotSelect = SelectState<SearchableVec<String>>;

type ProfileSelect = SelectState<SearchableVec<ProfileItem>>;

/// A field and the value it was last filled with; `None` refills it.
struct Line {
    state: Entity<InputState>,
    filled: Option<String>,
}

struct Block {
    state: Entity<TextareaState>,
    filled: Option<String>,
}

/// The panel's controls, made on its first render because inputs need a
/// window.
struct Fields {
    node_id: Line,
    node_slot: Entity<SlotSelect>,
    node_prompt: Block,
    slot_name: Line,
    slot_profile: Entity<ProfileSelect>,
    slot_role: Block,
    settings: Vec<Line>,
    max_parallel: Line,
    slot_items: Vec<String>,
    profile_items: Vec<ProfileItem>,
    _subscriptions: Vec<Subscription>,
}

pub(super) struct Properties {
    fields: Option<Fields>,

    /// The node the node fields show, by id; their edits apply to it.
    node: Option<String>,

    /// The slot the slot fields show, by name.
    slot: Option<String>,
}

impl Properties {
    pub(super) fn new() -> Self {
        Self {
            fields: None,
            node: None,
            slot: None,
        }
    }

    /// Refill `field` from the definition on the next render, as after an
    /// edit the definition refused. Only that field is refilled, so the one
    /// focus moved to keeps its caret.
    pub(super) fn refill(&mut self, field: Field) {
        let Some(fields) = &mut self.fields else {
            return;
        };

        let filled = match field {
            Field::NodeId => &mut fields.node_id.filled,
            Field::NodePrompt => &mut fields.node_prompt.filled,
            Field::SlotName => &mut fields.slot_name.filled,
            Field::SlotRole => &mut fields.slot_role.filled,
            Field::MaxParallel => &mut fields.max_parallel.filled,
            Field::Setting(index) => match fields.settings.get_mut(index) {
                Some(line) => &mut line.filled,
                None => return,
            },
        };

        *filled = None;
    }

    pub(super) fn select_slot(&mut self, slot: Option<String>) {
        self.slot = slot;
    }

    /// The edit a text field's value makes, if any.
    pub(super) fn edit_for(&self, field: Field, definition: &Definition, cx: &App) -> Option<Edit> {
        let fields = self.fields.as_ref()?;

        match field {
            Field::NodeId => {
                let from = self.node.clone()?;
                let to = fields.node_id.state.read(cx).text().to_string();
                let to = to.trim();

                (to != from).then(|| Edit::RenameNode {
                    from,
                    to: to.to_owned(),
                })
            }
            Field::NodePrompt => {
                let node = definition
                    .nodes
                    .iter()
                    .find(|node| Some(&node.id) == self.node.as_ref())?;

                let text = fields.node_prompt.state.read(cx).text().to_string();

                Some(Edit::SetNode {
                    id: node.id.clone(),
                    slot: node.slot.clone(),
                    prompt: (!text.trim().is_empty()).then_some(text),
                })
            }
            Field::SlotName => {
                let from = self.slot.clone()?;
                let to = fields.slot_name.state.read(cx).text().to_string();
                let to = to.trim();

                (to != from).then(|| Edit::RenameSlot {
                    from,
                    to: to.to_owned(),
                })
            }
            Field::SlotRole => {
                let mut body = self.slot_body(definition)?;

                body.role = fields.slot_role.state.read(cx).text().to_string();

                self.set_slot(body)
            }
            Field::Setting(index) => {
                let mut body = self.slot_body(definition)?;

                let text = fields
                    .settings
                    .get(index)?
                    .state
                    .read(cx)
                    .text()
                    .to_string();

                let text = text.trim();

                *setting_fields(&mut body.settings)[index].1 =
                    (!text.is_empty()).then(|| text.to_owned());

                self.set_slot(body)
            }
            Field::MaxParallel => {
                let text = fields.max_parallel.state.read(cx).text().to_string();
                let text = text.trim();

                if text.is_empty() {
                    return Some(Edit::SetMaxParallel(None));
                }

                text.parse()
                    .ok()
                    .map(|value| Edit::SetMaxParallel(Some(value)))
            }
        }
    }

    /// The edit a picked value makes.
    pub(super) fn edit_for_choice(&self, choice: Choice, definition: &Definition) -> Option<Edit> {
        match choice {
            Choice::NodeSlot(slot) => {
                let node = definition
                    .nodes
                    .iter()
                    .find(|node| Some(&node.id) == self.node.as_ref())?;

                Some(Edit::SetNode {
                    id: node.id.clone(),
                    slot,
                    prompt: node.prompt.clone(),
                })
            }
            Choice::SlotProfile(profile) => {
                let mut body = self.slot_body(definition)?;

                body.profile = profile;

                self.set_slot(body)
            }
        }
    }

    fn slot_body(&self, definition: &Definition) -> Option<SlotBody> {
        definition
            .slots
            .iter()
            .find(|slot| Some(&slot.name) == self.slot.as_ref())
            .map(|slot| slot.body.clone())
    }

    fn set_slot(&self, body: SlotBody) -> Option<Edit> {
        Some(Edit::SetSlot {
            name: self.slot.clone()?,
            body,
        })
    }

    fn fields(&mut self, window: &mut Window, cx: &mut Context<DefinitionEditor>) -> &mut Fields {
        self.fields.get_or_insert_with(|| make_fields(window, cx))
    }

    /// Fill the fields from `definition` for node `selected`, then draw the
    /// panel.
    pub(super) fn render(
        &mut self,
        definition: &Definition,
        errors: &[DefinitionError],
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<DefinitionEditor>,
    ) -> AnyElement {
        let node = selected.and_then(|index| definition.nodes.get(index));

        if node.map(|node| &node.id) != self.node.as_ref() {
            self.node = node.map(|node| node.id.clone());

            // Showing another node shows its slot too.
            if let Some(node) = node
                && definition.slots.iter().any(|slot| slot.name == node.slot)
            {
                self.slot = Some(node.slot.clone());
            }
        }

        let slot_exists = definition
            .slots
            .iter()
            .any(|slot| Some(&slot.name) == self.slot.as_ref());

        if !slot_exists {
            self.slot = definition.slots.first().map(|slot| slot.name.clone());
        }

        let slot = definition
            .slots
            .iter()
            .find(|slot| Some(&slot.name) == self.slot.as_ref())
            .cloned();

        let profiles: Vec<ProfileItem> = cx
            .global::<AgentSettings>()
            .profiles
            .iter()
            .map(|profile| ProfileItem {
                reference: ProfileReference {
                    kind: profile.kind,
                    name: profile.name.clone(),
                },
                title: format!("{} ({})", profile.name, profile.kind.full_name()).into(),
            })
            .collect();

        let slot_names: Vec<String> = definition
            .slots
            .iter()
            .map(|slot| slot.name.clone())
            .collect();

        let max_parallel = definition
            .max_parallel
            .map(|value| value.to_string())
            .unwrap_or_default();

        let fields = self.fields(window, cx);

        if let Some(node) = node {
            fill_line(&mut fields.node_id, &node.id, window, cx);

            fill_block(
                &mut fields.node_prompt,
                node.prompt.as_deref().unwrap_or_default(),
                window,
                cx,
            );
        }

        if fields.slot_items != slot_names {
            fields.slot_items.clone_from(&slot_names);

            fields.node_slot.update(cx, |select, cx| {
                select.set_items(SearchableVec::new(slot_names.clone()), window, cx)
            });
        }

        if let Some(node) = node
            && fields.node_slot.read(cx).selected_value() != Some(&node.slot)
        {
            fields.node_slot.update(cx, |select, cx| {
                select.set_selected_value(&node.slot, window, cx)
            });
        }

        if fields.profile_items != profiles {
            fields.profile_items.clone_from(&profiles);

            fields.slot_profile.update(cx, |select, cx| {
                select.set_items(SearchableVec::new(profiles.clone()), window, cx)
            });
        }

        if let Some(slot) = &slot {
            fill_line(&mut fields.slot_name, &slot.name, window, cx);
            fill_block(&mut fields.slot_role, &slot.body.role, window, cx);

            let mut settings = slot.body.settings.clone();

            for (line, (_, value)) in fields
                .settings
                .iter_mut()
                .zip(setting_fields(&mut settings))
            {
                fill_line(line, value.as_deref().unwrap_or_default(), window, cx);
            }

            if fields.slot_profile.read(cx).selected_value() != Some(&slot.body.profile) {
                fields.slot_profile.update(cx, |select, cx| {
                    select.set_selected_value(&slot.body.profile, window, cx)
                });
            }
        }

        fill_line(&mut fields.max_parallel, &max_parallel, window, cx);

        let marked_slots: Vec<&str> = errors.iter().flat_map(DefinitionError::slots).collect();

        let references = selected
            .map(|index| ancestors(definition, index))
            .unwrap_or_default();

        let node_section = node.map(|node| self.render_node(&node.id, references, cx));
        let slots_section = self.render_slots(definition, &marked_slots, slot.is_some(), cx);
        let settings_section = self.render_settings(cx);

        v_flex()
            .id("orchestration-properties")
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_none()
            .p_2()
            .gap_3()
            .border_l_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll()
            .children(node_section)
            .child(slots_section)
            .child(settings_section)
            .into_any_element()
    }

    fn render_node(
        &self,
        id: &str,
        references: Vec<String>,
        cx: &mut Context<DefinitionEditor>,
    ) -> AnyElement {
        let Some(fields) = &self.fields else {
            return div().into_any_element();
        };

        let prompt = fields.node_prompt.state.clone();

        let theme = cx.theme();
        let (border, accent) = (theme.border, theme.accent);

        let chips = iter::once("{{input}}".to_owned())
            .chain(
                references
                    .into_iter()
                    .map(|ancestor| format!("{{{{{ancestor}.output}}}}")),
            )
            .enumerate()
            .map(|(index, reference)| {
                let prompt = prompt.clone();
                let text = reference.clone();

                div()
                    .id(("orchestration-reference", index))
                    .px_1()
                    .rounded(UI_RADIUS)
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .cursor_pointer()
                    .hover(|chip| chip.bg(accent))
                    .on_click(move |_, window, cx| {
                        prompt.update(cx, |prompt, cx| {
                            prompt.insert(text.clone(), window, cx);

                            prompt.focus(window, cx);
                        })
                    })
                    .child(reference)
            });

        v_flex()
            .gap_1()
            .child(heading(
                t!("orchestration-node-properties").into_owned(),
                id,
            ))
            .child(label(t!("orchestration-node-id").into_owned(), cx))
            .child(Input::new(&fields.node_id.state).small())
            .child(label(t!("orchestration-node-slot").into_owned(), cx))
            .child(Select::new(&fields.node_slot).small())
            .child(label(t!("orchestration-node-prompt").into_owned(), cx))
            .child(Textarea::new(&fields.node_prompt.state))
            .child(label(t!("orchestration-insert-reference").into_owned(), cx))
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .into_any_element()
    }

    fn render_slots(
        &self,
        definition: &Definition,
        marked: &[&str],
        has_slot: bool,
        cx: &mut Context<DefinitionEditor>,
    ) -> AnyElement {
        let Some(fields) = &self.fields else {
            return div().into_any_element();
        };

        let theme = cx.theme();

        let chips: Vec<AnyElement> = definition
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let name = slot.name.clone();
                let active = Some(&slot.name) == self.slot.as_ref();

                let color = if marked.contains(&slot.name.as_str()) {
                    theme.danger
                } else {
                    theme.foreground
                };

                div()
                    .id(("orchestration-slot-chip", index))
                    .px_1()
                    .rounded(UI_RADIUS)
                    .border_1()
                    .border_color(if active { theme.primary } else { theme.border })
                    .text_xs()
                    .text_color(color)
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |editor, _, _, cx| editor.show_slot(name.clone(), cx)),
                    )
                    .child(slot.name.clone())
                    .into_any_element()
            })
            .collect();

        let settings: Vec<AnyElement> = SETTING_KEYS
            .iter()
            .zip(&fields.settings)
            .map(|(key, line)| {
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .w(px(110.))
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(*key),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&line.state).xsmall()),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(heading(t!("orchestration-slots").into_owned(), ""))
                    .child(
                        Button::new("orchestration-add-slot")
                            .ghost()
                            .xsmall()
                            .label(t!("orchestration-add-slot"))
                            .on_click(cx.listener(|editor, _, _, cx| editor.add_slot(cx))),
                    ),
            )
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .when(has_slot, |section| {
                section
                    .child(label(t!("orchestration-slot-name").into_owned(), cx))
                    .child(Input::new(&fields.slot_name.state).small())
                    .child(label(t!("orchestration-slot-profile").into_owned(), cx))
                    .child(Select::new(&fields.slot_profile).small())
                    .child(label(t!("orchestration-slot-role").into_owned(), cx))
                    .child(Textarea::new(&fields.slot_role.state))
                    .child(label(t!("orchestration-slot-settings").into_owned(), cx))
                    .children(settings)
                    .child(
                        Button::new("orchestration-remove-slot")
                            .ghost()
                            .xsmall()
                            .label(t!("orchestration-remove-slot"))
                            .disabled(self.slot.is_none())
                            .on_click(cx.listener(|editor, _, _, cx| editor.remove_slot(cx))),
                    )
            })
            .into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<DefinitionEditor>) -> AnyElement {
        let Some(fields) = &self.fields else {
            return div().into_any_element();
        };

        v_flex()
            .gap_1()
            .child(heading(
                t!("orchestration-definition-settings").into_owned(),
                "",
            ))
            .child(label(t!("orchestration-max-parallel").into_owned(), cx))
            .child(Input::new(&fields.max_parallel.state).small())
            .into_any_element()
    }

    pub(super) fn slot(&self) -> Option<&str> {
        self.slot.as_deref()
    }
}

fn make_fields(window: &mut Window, cx: &mut Context<DefinitionEditor>) -> Fields {
    let mut subscriptions = Vec::new();

    let mut line = |field: Field,
                    placeholder: String,
                    window: &mut Window,
                    cx: &mut Context<DefinitionEditor>| {
        let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));

        subscriptions.push(cx.subscribe_in(
            &state,
            window,
            move |editor, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    editor.commit(field, cx);
                }
            },
        ));

        Line {
            state,
            filled: None,
        }
    };

    let node_id = line(Field::NodeId, String::new(), window, cx);
    let slot_name = line(Field::SlotName, String::new(), window, cx);

    let settings = (0..SETTING_KEYS.len())
        .map(|index| {
            line(
                Field::Setting(index),
                t!("orchestration-setting-from-profile").into_owned(),
                window,
                cx,
            )
        })
        .collect();

    let max_parallel = line(
        Field::MaxParallel,
        t!("orchestration-max-parallel-default").into_owned(),
        window,
        cx,
    );

    let mut block = |field: Field, window: &mut Window, cx: &mut Context<DefinitionEditor>| {
        let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 10));

        subscriptions.push(cx.subscribe_in(
            &state,
            window,
            move |editor, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur) {
                    editor.commit(field, cx);
                }
            },
        ));

        Block {
            state,
            filled: None,
        }
    };

    let node_prompt = block(Field::NodePrompt, window, cx);
    let slot_role = block(Field::SlotRole, window, cx);

    let node_slot = cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx));

    let slot_profile =
        cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx));

    subscriptions.push(cx.subscribe_in(
        &node_slot,
        window,
        |editor, _, event: &SelectEvent<SearchableVec<String>>, _, cx| {
            if let SelectEvent::Confirm(Some(slot)) = event {
                editor.choose(Choice::NodeSlot(slot.clone()), cx);
            }
        },
    ));

    subscriptions.push(cx.subscribe_in(
        &slot_profile,
        window,
        |editor, _, event: &SelectEvent<SearchableVec<ProfileItem>>, _, cx| {
            if let SelectEvent::Confirm(Some(profile)) = event {
                editor.choose(Choice::SlotProfile(profile.clone()), cx);
            }
        },
    ));

    Fields {
        node_id,
        node_slot,
        node_prompt,
        slot_name,
        slot_profile,
        slot_role,
        settings,
        max_parallel,
        slot_items: Vec::new(),
        profile_items: Vec::new(),
        _subscriptions: subscriptions,
    }
}

fn fill_line(line: &mut Line, value: &str, window: &mut Window, cx: &mut App) {
    if line.filled.as_deref() != Some(value) {
        line.filled = Some(value.to_owned());

        line.state.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }
}

fn fill_block(block: &mut Block, value: &str, window: &mut Window, cx: &mut App) {
    if block.filled.as_deref() != Some(value) {
        block.filled = Some(value.to_owned());

        block.state.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx)
        });
    }
}

/// Each thread setting by its key, borrowed for reading or writing.
fn setting_fields(settings: &mut ThreadSettings) -> [(&'static str, &mut Option<String>); 7] {
    [
        (SETTING_KEYS[0], &mut settings.model),
        (SETTING_KEYS[1], &mut settings.approval),
        (SETTING_KEYS[2], &mut settings.approvals_reviewer),
        (SETTING_KEYS[3], &mut settings.sandbox),
        (SETTING_KEYS[4], &mut settings.effort),
        (SETTING_KEYS[5], &mut settings.tier),
        (SETTING_KEYS[6], &mut settings.agent_preset),
    ]
}

/// The ids of every node `node` depends on, directly or not, in definition
/// order: the outputs its prompt may read. A cycle back to `node` is not
/// an ancestor of it.
pub(super) fn ancestors(definition: &Definition, node: usize) -> Vec<String> {
    let mut found = vec![false; definition.nodes.len()];
    let mut pending = vec![node];

    while let Some(current) = pending.pop() {
        for dependency in &definition.nodes[current].needs {
            if let Some(index) = definition
                .nodes
                .iter()
                .position(|node| &node.id == dependency)
                && index != node
                && !found[index]
            {
                found[index] = true;

                pending.push(index);
            }
        }
    }

    definition
        .nodes
        .iter()
        .zip(found)
        .filter(|(_, found)| *found)
        .map(|(node, _)| node.id.clone())
        .collect()
}

fn heading(title: String, detail: &str) -> impl IntoElement {
    h_flex()
        .flex_1()
        .gap_1()
        .text_sm()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().truncate().child(detail.to_owned()))
}

fn label(text: String, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

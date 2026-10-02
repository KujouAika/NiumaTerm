use std::cell::Cell;
use std::rc::Rc;

use gpui::px;
use nmt_config::appearance::TabShape;

use crate::ui::tab_bar::horizontal::shell_tab;

struct TabGestureProbe {
    parent_presses: Rc<Cell<usize>>,
}

impl gpui::Render for TabGestureProbe {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use crate::ui::tab_bar::drag::{DragLabelPreview, DragStyle, TabDrag};
        use gpui::prelude::*;
        use gpui::{MouseButton, div};

        let presses = self.parent_presses.clone();

        div()
            .size_full()
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                presses.set(presses.get() + 1);
            })
            .child(
                shell_tab(TabShape::Attached)
                    .w(px(160.))
                    .label("Codex")
                    .on_drag(TabDrag { from: 0 }, |_, _, _, cx| {
                        cx.new(|_| DragLabelPreview {
                            style: DragStyle::Tab,
                            label: "Codex".into(),
                            width: 160.,
                        })
                    }),
            )
    }
}

#[gpui::test]
fn tab_press_stays_out_of_titlebar_while_reorder_drag_still_starts(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point};
    use std::cell::Cell;
    use std::rc::Rc;

    cx.update(gpui_component::init);

    let parent_presses = Rc::new(Cell::new(0));

    let handle = cx.add_window({
        let parent_presses = parent_presses.clone();

        move |_, _| TabGestureProbe { parent_presses }
    });

    let mut cx = VisualTestContext::from_window(handle.into(), cx);

    cx.refresh().unwrap();

    cx.simulate_mouse_down(
        point(px(70.), px(15.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(parent_presses.get(), 0);

    cx.simulate_mouse_move(
        point(px(90.), px(15.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert!(cx.update(|_, cx| cx.has_active_drag()));
    assert_eq!(parent_presses.get(), 0);

    cx.simulate_mouse_up(
        point(px(90.), px(15.)),
        MouseButton::Left,
        Modifiers::default(),
    );

    cx.simulate_click(point(px(220.), px(15.)), Modifiers::default());

    assert_eq!(parent_presses.get(), 1);
}

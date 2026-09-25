#[gpui::test]
fn closing_retains_the_image_and_reset_discards_the_preview(cx: &mut gpui::TestAppContext) {
    use std::io::Cursor;
    use std::sync::Arc;

    use gpui::{AppContext as _, Image, ImageFormat};

    use crate::agent_tab::AgentKind;
    use crate::agent_tab::settings::AgentSettings;
    use crate::agent_tab::transcript::TranscriptView;
    use crate::agent_tab::transcript::render::image_preview::ImagePreview;

    let mut bytes = Cursor::new(Vec::new());

    image_rs::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut bytes, image_rs::ImageFormat::Png)
        .unwrap();

    let image = Arc::new(Image::from_bytes(ImageFormat::Png, bytes.into_inner()));

    cx.set_global(AgentSettings::default());

    let cx = cx.add_empty_window();
    let view = cx.update(|_, cx| cx.new(|_| TranscriptView::new(AgentKind::Codex, None)));

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.zoom_image(image.clone(), None, cx);

            view.preview.close_zoomed_image(cx);

            assert!(
                matches!(&view.preview.image_preview, ImagePreview::Closing(preview)
            if Arc::ptr_eq(&preview.image, &image))
            );

            view.zoom_image(image.clone(), None, cx);

            view.reset_presentation();

            assert!(matches!(view.preview.image_preview, ImagePreview::Closed));
        })
    });
}

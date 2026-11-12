//! Image diffs — "before" and "after" side by side, on a checkerboard so
//! transparency is visible. SVG can also be viewed as a text diff.

use std::path::PathBuf;

use gitkraft_core::{DiffInfo, FileStatus, ImageKind};
use iced::widget::{Space, canvas, column, container, row, stack, text};
use iced::{Alignment, ContentFit, Element, Font, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::macros::StringErr;
use crate::message::Message;
use crate::theme;
use crate::theme::ThemeColors;

/// Identity of an image diff: changes whenever either side's content does.
pub(crate) fn image_key(diff: &DiffInfo) -> Option<String> {
    gitkraft_core::image_kind(diff.display_path())?;
    Some(format!(
        "{}|{}|{}|{}",
        diff.display_path(),
        diff.status,
        diff.old_id.as_deref().unwrap_or("-"),
        diff.new_id.as_deref().unwrap_or("workdir"),
    ))
}

/// A loaded image diff, ready to render.
#[derive(Debug, Clone)]
pub struct ImagePreview {
    /// [`image_key`] of the diff this preview belongs to.
    pub key: String,
    pub result: Result<ImagePair, String>,
}

#[derive(Debug, Clone)]
pub struct ImagePair {
    pub kind: ImageKind,
    pub old: Option<ImageSide>,
    pub new: Option<ImageSide>,
}

#[derive(Debug, Clone)]
pub struct ImageSide {
    pub handle: SideHandle,
    pub bytes: usize,
    /// Pixel size for bitmaps; `None` for SVG or undecodable data.
    pub dimensions: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub enum SideHandle {
    Raster(iced::widget::image::Handle),
    Svg(iced::widget::svg::Handle),
}

impl ImageSide {
    fn new(kind: ImageKind, bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        match kind {
            ImageKind::Raster => Self {
                dimensions: raster_dimensions(&bytes),
                handle: SideHandle::Raster(iced::widget::image::Handle::from_bytes(bytes)),
                bytes: len,
            },
            ImageKind::Svg => Self {
                dimensions: None,
                handle: SideHandle::Svg(iced::widget::svg::Handle::from_memory(bytes)),
                bytes: len,
            },
        }
    }
}

fn raster_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Load both sides of `diff` on a background thread.
pub(crate) fn load_image_preview(
    path: PathBuf,
    key: String,
    diff: DiffInfo,
) -> iced::Task<Message> {
    let task_key = key.clone();
    iced::Task::perform(
        async move {
            let (tx, rx) = futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let result = (|| {
                    let repo = gitkraft_core::features::repo::open_repo(&path).str_err()?;
                    let img =
                        gitkraft_core::features::diff::load_image_diff(&repo, &diff).str_err()?;
                    Ok(ImagePair {
                        kind: img.kind,
                        old: img.old.map(|b| ImageSide::new(img.kind, b)),
                        new: img.new.map(|b| ImageSide::new(img.kind, b)),
                    })
                })();
                let _ = tx.send(result);
            });
            rx.await
                .unwrap_or_else(|_| Err("Task cancelled".to_string()))
        },
        move |result| {
            Message::ImagePreviewLoaded(ImagePreview {
                key: task_key.clone(),
                result,
            })
        },
    )
}

/// The body of the diff panel for an image file.
pub(crate) fn view<'a>(
    preview: Option<&'a ImagePreview>,
    status: &FileStatus,
    c: &ThemeColors,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = match preview.map(|p| &p.result) {
        None => centered(text("Loading image…").size(13).color(c.muted)),
        Some(Err(e)) => centered(
            text(format!("Cannot show image: {e}"))
                .size(13)
                .color(c.red),
        ),
        Some(Ok(pair)) => {
            // An added file has no "before" and a deleted one no "after":
            // show only the side that exists.
            let (show_before, show_after) = match status {
                FileStatus::New | FileStatus::Untracked => (false, true),
                FileStatus::Deleted => (true, false),
                _ => (true, true),
            };
            let mut sides = row![].spacing(12).width(Length::Fill).height(Length::Fill);
            if show_before {
                sides = sides.push(side_panel("Before", pair.old.as_ref(), c, c.red));
            }
            if show_after {
                sides = sides.push(side_panel("After", pair.new.as_ref(), c, c.green));
            }
            sides.into()
        }
    };
    container(body)
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn centered<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn side_panel<'a>(
    label: &'static str,
    side: Option<&'a ImageSide>,
    c: &ThemeColors,
    accent: iced::Color,
) -> Element<'a, Message> {
    let info = match side {
        Some(s) => {
            let size = human_size(s.bytes);
            match s.dimensions {
                Some((w, h)) => format!("{w} × {h} px · {size}"),
                None => size,
            }
        }
        None => String::new(),
    };
    let header = row![
        text(label).size(12).color(accent),
        Space::new().width(8),
        text(info).size(11).color(c.muted).font(Font::MONOSPACE),
    ]
    .align_y(Alignment::Center);

    let picture: Element<'a, Message> = match side {
        Some(s) => {
            let img: Element<'a, Message> = match &s.handle {
                SideHandle::Raster(h) => iced::widget::image(h.clone())
                    .content_fit(ContentFit::ScaleDown)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                SideHandle::Svg(h) => iced::widget::svg(h.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
            };
            stack![
                canvas(Checkerboard {
                    light: c.surface_highlight,
                    dark: c.surface,
                })
                .width(Length::Fill)
                .height(Length::Fill),
                container(img)
                    .padding(8)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
        None => centered(text("No image").size(12).color(c.muted)),
    };

    column![
        header,
        Space::new().height(6),
        container(picture)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::header_style),
    ]
    .width(Length::FillPortion(1))
    .height(Length::Fill)
    .into()
}

fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

/// Two-tone checkerboard so transparent pixels are visible on any theme.
#[derive(Debug)]
struct Checkerboard {
    light: iced::Color,
    dark: iced::Color,
}

impl canvas::Program<Message> for Checkerboard {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        const CELL: f32 = 10.0;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), self.dark);
        let cols = (bounds.width / CELL).ceil() as usize;
        let rows = (bounds.height / CELL).ceil() as usize;
        for y in 0..rows {
            for x in (y % 2..cols).step_by(2) {
                frame.fill_rectangle(
                    Point::new(x as f32 * CELL, y as f32 * CELL),
                    Size::new(CELL, CELL),
                    self.light,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(path: &str, old: Option<&str>, new: Option<&str>) -> DiffInfo {
        DiffInfo {
            old_file: path.into(),
            new_file: path.into(),
            status: FileStatus::Modified,
            hunks: Vec::new(),
            old_id: old.map(Into::into),
            new_id: new.map(Into::into),
        }
    }

    #[test]
    fn key_only_for_images() {
        assert!(image_key(&diff("src/main.rs", None, None)).is_none());
        assert!(image_key(&diff("logo.png", Some("a"), Some("b"))).is_some());
    }

    #[test]
    fn key_changes_with_content() {
        let a = image_key(&diff("logo.svg", Some("a"), Some("b")));
        let b = image_key(&diff("logo.svg", Some("a"), Some("c")));
        assert_ne!(a, b);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn png_dimensions_are_read() {
        // 1×1 transparent PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        assert_eq!(raster_dimensions(png), Some((1, 1)));
        assert_eq!(raster_dimensions(b"not an image"), None);
    }
}

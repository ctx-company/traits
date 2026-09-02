//! The one measured overflow container the desktop reuses: a clipped
//! viewport around arbitrary content, plus a canvas gradient overlay with a
//! centred "scroll to end" label that paints only when the content's
//! resolved height is strictly greater than the viewport's resolved height.
//! Equality is fit; no valid measurement paints nothing. A custom
//! [`Element`] decides this after layout, in the same frame — no cached
//! measurement, no `cx.notify()`, nothing to go stale across an
//! overflow/fit transition.

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Style, Window, div, linear_color_stop, linear_gradient, point,
    px, rgb, size,
};

use crate::tokens;

const SCROLL_HINT: &str = "scroll to end";

fn transparent_canvas() -> gpui::Hsla {
    let mut color: gpui::Hsla = rgb(tokens::CANVAS).into();
    color.a = 0.;
    color
}

fn overlay_element(selector: &'static str) -> AnyElement {
    div()
        // `debug_selector` is a gpui-provided no-op outside test/test-support
        // builds; it is how a runtime proof reads this element's actual
        // painted bounds (`VisualTestContext::debug_bounds`) rather than a
        // caller-recomputed proxy of what `prepaint` intended to paint.
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_row()
        .justify_center()
        .items_end()
        .w_full()
        .h(tokens::BOTTOM_FADE_HEIGHT)
        .pb(tokens::BOTTOM_FADE_LABEL_PAD_BOTTOM)
        .bg(linear_gradient(
            tokens::BOTTOM_FADE_ANGLE_DEG,
            linear_color_stop(transparent_canvas(), 0.),
            linear_color_stop(rgb(tokens::CANVAS), tokens::BOTTOM_FADE_OPAQUE_STOP),
        ))
        .child(
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_FAINT))
                .whitespace_nowrap()
                .child(SCROLL_HINT),
        )
        .into_any_element()
}

/// The pure visibility decision, kept separate from layout/paint so it is
/// directly unit-testable. Equality is fit; a viewport with no valid
/// measurement (zero height) never overflows.
fn overflows(viewport_height: Pixels, content_height: Pixels) -> bool {
    viewport_height > px(0.) && content_height > viewport_height
}

/// Wrap `content` in a clipped viewport that fades and labels its bottom
/// edge only when `content` is taller than the viewport gpui resolves for
/// it. No width, no height, no overflow flag, no content-height parameter —
/// the caller supplies content and placement; this owns the measurement and
/// the visibility decision.
pub fn clipped_with_overflow_fade(content: AnyElement) -> impl IntoElement {
    clipped_with_named_overflow_fade(content, "overflow-fade-overlay")
}

/// As [`clipped_with_overflow_fade`], with a caller-owned selector for a
/// composition that renders more than one overflow-capable region.
pub fn clipped_with_named_overflow_fade(
    content: AnyElement,
    overlay_selector: &'static str,
) -> impl IntoElement {
    OverflowFade {
        content: div()
            .debug_selector(|| "overflow-fade-content".to_string())
            .flex_shrink_0()
            .w_full()
            .child(content)
            .into_any_element(),
        overlay_selector,
    }
}

struct OverflowFade {
    content: AnyElement,
    overlay_selector: &'static str,
}

/// What `prepaint` resolved: the measured content height, and — only when
/// `content` overflows the viewport — the overlay element and the bounds it
/// was laid out at, ready for `paint` to actually paint.
struct Prepainted {
    #[cfg_attr(not(test), allow(dead_code))]
    content_height: Pixels,
    overlay: Option<(AnyElement, Bounds<Pixels>)>,
}

impl IntoElement for OverflowFade {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OverflowFade {
    type RequestLayoutState = LayoutId;
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let content_layout_id = self.content.request_layout(window, cx);
        let size = gpui::Size {
            width: gpui::relative(1.).into(),
            ..gpui::Size::auto()
        };
        let min_size = gpui::Size {
            height: px(0.).into(),
            ..gpui::Size::auto()
        };
        let overflow = gpui::Point {
            y: gpui::Overflow::Hidden,
            ..gpui::Point::default()
        };
        let style = Style {
            display: gpui::Display::Flex,
            flex_direction: gpui::FlexDirection::Column,
            flex_grow: 1.,
            flex_basis: px(0.).into(),
            size,
            min_size,
            overflow,
            ..Style::default()
        };
        let layout_id = window.request_layout(style, [content_layout_id], cx);
        (layout_id, content_layout_id)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        content_layout_id: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let content_height = window.layout_bounds(*content_layout_id).size.height;
        let overflows = overflows(bounds.size.height, content_height);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.content.prepaint(window, cx);
        });

        let overlay = if overflows {
            let overlay_bounds = Bounds {
                origin: point(
                    bounds.origin.x,
                    bounds.bottom() - tokens::BOTTOM_FADE_HEIGHT,
                ),
                size: size(bounds.size.width, tokens::BOTTOM_FADE_HEIGHT),
            };

            let mut overlay = overlay_element(self.overlay_selector);
            overlay.layout_as_root(
                size(
                    gpui::AvailableSpace::Definite(bounds.size.width),
                    gpui::AvailableSpace::Definite(tokens::BOTTOM_FADE_HEIGHT),
                ),
                window,
                cx,
            );
            overlay.prepaint_at(overlay_bounds.origin, window, cx);
            Some((overlay, overlay_bounds))
        } else {
            None
        };

        Prepainted {
            content_height,
            overlay,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _content_layout_id: &mut Self::RequestLayoutState,
        prepainted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        let mut content_mask = None;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            #[cfg(test)]
            {
                content_mask = Some(window.content_mask());
            }
            self.content.paint(window, cx);
        });

        // The overlay only actually reaches the screen through this branch —
        // `overlay.paint` is what puts it in the frame's scene. The test
        // observation below is recorded from what this branch actually did,
        // not from the geometry `prepaint` computed.
        #[cfg_attr(not(test), allow(unused_variables))]
        let painted_overlay = if let Some((overlay, overlay_bounds)) = prepainted.overlay.as_mut() {
            overlay.paint(window, cx);
            Some(*overlay_bounds)
        } else {
            None
        };

        #[cfg(test)]
        test_observer::record(test_observer::Observation {
            viewport: bounds,
            content_height: prepainted.content_height,
            content_mask: content_mask.expect("the content mask closure always runs"),
            overlay: painted_overlay,
        });
    }
}

/// A `#[cfg(test)]`-only side channel recording each frame's resolved
/// geometry and overlay decision, so runtime proofs can assert on what the
/// container measured and painted rather than only that it did not panic.
/// Test-only and thread-local: adds nothing to the production `Element`
/// state or to `clipped_with_overflow_fade`'s signature.
#[cfg(test)]
mod test_observer {
    use gpui::{Bounds, ContentMask, Pixels};
    use std::cell::RefCell;

    #[derive(Debug, Clone)]
    pub(super) struct Observation {
        pub viewport: Bounds<Pixels>,
        pub content_height: Pixels,
        /// The content mask actually in effect while `content` painted, read
        /// live from `Window::content_mask` inside the masked closure — the
        /// mask gpui really applied this frame, not a value this element
        /// re-derives from `bounds`.
        pub content_mask: ContentMask<Pixels>,
        pub overlay: Option<Bounds<Pixels>>,
    }

    thread_local! {
        static LAST: RefCell<Option<Observation>> = const { RefCell::new(None) };
    }

    pub(super) fn record(observation: Observation) {
        LAST.with(|cell| *cell.borrow_mut() = Some(observation));
    }

    pub(super) fn take() -> Option<Observation> {
        LAST.with(|cell| cell.borrow_mut().take())
    }
}

#[cfg(test)]
pub(crate) fn take_last_measurement() -> Option<(f32, f32)> {
    test_observer::take().map(|observation| {
        (
            f32::from(observation.viewport.size.height),
            f32::from(observation.content_height),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail_tree::{self, ActivityOverlay};
    use crate::frame_list::FrameList;
    use crate::frame_list_view::{frame_list_element, session_with_all_four_forms};

    #[test]
    fn overflows_is_strict_and_treats_zero_height_as_no_measurement() {
        assert!(!overflows(px(100.), px(99.)));
        assert!(!overflows(px(100.), px(100.)), "equality is fit");
        assert!(overflows(px(100.), px(100.001)));
        assert!(
            !overflows(px(0.), px(50.)),
            "no valid measurement paints nothing"
        );
    }

    #[test]
    fn tokens_reproduce_the_export_gradient_and_height() {
        assert_eq!(tokens::BOTTOM_FADE_OPAQUE_STOP, 0.8);
        assert_eq!(tokens::BOTTOM_FADE_HEIGHT, px(100.));
    }

    /// Content of a known, fixed height — the fit/exact-fit/overflow matrix
    /// needs an exact comparison the real text metrics can't offer (`risk
    /// 3` in the plan: float equality through wrapped text is unreachable).
    /// The viewport is `size_full()`: it tracks the real window bounds, so a
    /// `simulate_window_resize` genuinely changes what the container
    /// measures rather than a field this probe ignores.
    struct OverflowFadeProbe {
        content_height: Pixels,
    }

    impl gpui::Render for OverflowFadeProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(clipped_with_overflow_fade(
                    div().h(self.content_height).w_full().into_any_element(),
                ))
        }
    }

    fn open_fixed_height_probe(
        cx: &mut gpui::TestAppContext,
        content_height: Pixels,
        viewport_height: Pixels,
    ) -> gpui::WindowHandle<OverflowFadeProbe> {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::centered(None, size(px(300.), viewport_height), cx);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| OverflowFadeProbe { content_height }),
                )
            })
            .expect("open a real window");
        cx.run_until_parked();
        window
    }

    /// Content shorter than the viewport: no overlay, and the recorded
    /// geometry is the actual resolved viewport/content heights, not just
    /// "did not panic". `debug_bounds` is gpui's own paint-phase record —
    /// `Div::paint` only inserts into it when that div actually paints — so
    /// its absence here is evidence the overlay div never reached `paint`,
    /// not a value this element computed and asserted against itself.
    #[gpui::test]
    fn fits_when_content_is_shorter_than_the_viewport(cx: &mut gpui::TestAppContext) {
        let window = open_fixed_height_probe(cx, px(50.), px(100.));
        let observation = test_observer::take().expect("prepaint runs at least once");
        assert_eq!(observation.viewport.size.height, px(100.));
        assert_eq!(observation.content_height, px(50.));
        assert!(observation.overlay.is_none());
        assert_eq!(
            observation.content_mask.bounds, observation.viewport,
            "the content mask actually applied during paint is the viewport's bounds"
        );

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(
            vcx.debug_bounds("overflow-fade-overlay").is_none(),
            "the overlay div never painted, so gpui recorded no bounds for it"
        );
    }

    /// Equality is fit: content exactly the viewport's height paints no
    /// overlay.
    #[gpui::test]
    fn exact_fit_paints_no_overlay(cx: &mut gpui::TestAppContext) {
        let window = open_fixed_height_probe(cx, px(100.), px(100.));
        let observation = test_observer::take().expect("prepaint runs at least once");
        assert_eq!(observation.content_height, observation.viewport.size.height);
        assert!(observation.overlay.is_none(), "equality is fit");

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(
            vcx.debug_bounds("overflow-fade-overlay").is_none(),
            "the overlay div never painted, so gpui recorded no bounds for it"
        );
    }

    /// Content taller than the viewport: the overlay is recorded with
    /// geometry derived from the measured viewport box — never a literal —
    /// and `debug_bounds` confirms the overlay div's *actual painted*
    /// bounds (gpui records these from inside `Div::paint`) agree with what
    /// this element intended to paint.
    #[gpui::test]
    fn overflow_paints_an_overlay_sized_and_placed_from_the_measured_viewport(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_fixed_height_probe(cx, px(150.), px(100.));
        let observation = test_observer::take().expect("prepaint runs at least once");
        let overlay = observation.overlay.expect("content overflows the viewport");
        assert_eq!(overlay.size.width, observation.viewport.size.width);
        assert_eq!(overlay.size.height, tokens::BOTTOM_FADE_HEIGHT);
        assert_eq!(overlay.origin.x, observation.viewport.origin.x);
        assert_eq!(
            overlay.origin.y + overlay.size.height,
            observation.viewport.origin.y + observation.viewport.size.height,
            "the overlay's bottom edge is the viewport's bottom edge"
        );
        assert_eq!(
            observation.content_mask.bounds, observation.viewport,
            "the content mask actually applied during paint is the viewport's bounds"
        );

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        let painted_overlay = vcx
            .debug_bounds("overflow-fade-overlay")
            .expect("the overlay div actually painted this frame");
        assert_eq!(
            painted_overlay, overlay,
            "gpui's own paint-time record of the overlay div agrees with what this element computed"
        );
        let painted_content = vcx
            .debug_bounds("overflow-fade-content")
            .expect("the content div actually painted this frame");
        assert_eq!(
            painted_content.size.height, observation.content_height,
            "the content div's actual painted height is the unclamped content height"
        );
    }

    /// A resize crossing the fit/overflow boundary in both directions: the
    /// decision is re-derived from a fresh measurement every frame, so no
    /// stale overlay survives a transition either way.
    #[gpui::test]
    fn resize_transitions_the_overlay_present_absent_and_back(cx: &mut gpui::TestAppContext) {
        let window = open_fixed_height_probe(cx, px(150.), px(500.));
        assert!(
            test_observer::take()
                .expect("initial frame")
                .overlay
                .is_none(),
            "tall viewport, fixed content: fits"
        );

        cx.simulate_window_resize(window.into(), size(px(300.), px(80.)));
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("resized frame")
                .overlay
                .is_some(),
            "shrinking the viewport below the content must produce an overlay"
        );

        cx.simulate_window_resize(window.into(), size(px(300.), px(500.)));
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("resized back frame")
                .overlay
                .is_none(),
            "growing the viewport back past the content must drop the overlay, not leave it stale"
        );
    }

    /// Same row count, a fixed viewport: content height alone flips the
    /// decision, in both directions, with no stale overlay surviving the
    /// transition.
    #[gpui::test]
    fn content_height_change_at_a_fixed_row_count_flips_the_decision(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::centered(None, size(px(300.), px(100.)), cx);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| OverflowFadeProbe {
                            content_height: px(50.),
                        })
                    },
                )
            })
            .expect("open a real window");
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("initial frame")
                .overlay
                .is_none(),
            "content shorter than the fixed viewport fits"
        );

        window
            .update(cx, |probe, _window, cx| {
                probe.content_height = px(150.);
                cx.notify();
            })
            .expect("update the probe entity");
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("grown-content frame")
                .overlay
                .is_some(),
            "growing the content past the fixed viewport must produce an overlay"
        );

        window
            .update(cx, |probe, _window, cx| {
                probe.content_height = px(50.);
                cx.notify();
            })
            .expect("update the probe entity");
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("shrunk-content frame")
                .overlay
                .is_none(),
            "shrinking the content back below the viewport must drop the overlay"
        );
    }

    struct RealFrameListProbe {
        list: FrameList,
        viewport_height: Pixels,
    }

    impl gpui::Render for RealFrameListProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .w(px(300.))
                .h(self.viewport_height)
                .child(clipped_with_overflow_fade(frame_list_element(&self.list)))
        }
    }

    fn real_frame_list() -> FrameList {
        let session = session_with_all_four_forms();
        let tree = detail_tree::project(&session, &ActivityOverlay::default(), true);
        FrameList::from_tree(&tree)
    }

    /// Real `FrameList` content — content of a shape the container does not
    /// know — in a short viewport overflows and in a tall one fits.
    #[gpui::test]
    fn real_frame_list_content_overflows_a_short_viewport_and_fits_a_tall_one(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(300.), px(60.)), cx);
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| RealFrameListProbe {
                        list: real_frame_list(),
                        viewport_height: px(60.),
                    })
                },
            )
        })
        .expect("open a real window");
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("short-viewport frame")
                .overlay
                .is_some(),
            "four real frame rows must overflow a 60px viewport"
        );

        cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(300.), px(2000.)), cx);
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| RealFrameListProbe {
                        list: real_frame_list(),
                        viewport_height: px(2000.),
                    })
                },
            )
        })
        .expect("open a real window");
        cx.run_until_parked();
        assert!(
            test_observer::take()
                .expect("tall-viewport frame")
                .overlay
                .is_none(),
            "the same four rows must fit a 2000px viewport"
        );
    }

    struct DetailAndBarProbe {
        tree: detail_tree::DetailTree,
        viewport_height: Pixels,
    }

    impl gpui::Render for DetailAndBarProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
            use crate::bottom_bar_view::bar_element;
            use crate::run_row::{StatePresentation, StateRole};

            let bar = BottomBar {
                state: StatePresentation {
                    word: "running",
                    role: StateRole::Accent,
                },
                detail: Vec::new(),
                actions: vec![BarAction {
                    id: BarActionId::WatchRaw,
                    label: "watch raw".to_string(),
                    tone: ActionTone::Secondary,
                }],
            };

            div()
                .id("detail-and-bar-probe")
                .flex()
                .flex_col()
                .w(px(300.))
                .h(self.viewport_height)
                .child(crate::detail_view::detail_element(&self.tree))
                .child(bar_element(&bar, None))
                .into_any_element()
        }
    }

    /// The detail pane's own composition (header + clipped frame list) with
    /// a bottom bar sibling below it: the frame section stays clipped
    /// (overlay geometry never exceeds the pane above the bar) and the bar
    /// paints beneath it, unobscured. The bar's bounds come from
    /// `debug_bounds` — gpui's own paint-phase record of where
    /// `bottom_bar_view::bar_element` actually painted, not a value this
    /// test or the container re-derives.
    #[gpui::test]
    fn detail_and_bar_composition_keeps_the_frame_section_clipped_above_the_bar(
        cx: &mut gpui::TestAppContext,
    ) {
        let session = session_with_all_four_forms();
        let tree = detail_tree::project(&session, &ActivityOverlay::default(), true);

        let window = cx
            .update(|cx| {
                let bounds = Bounds::centered(None, size(px(300.), px(120.)), cx);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| DetailAndBarProbe {
                            tree,
                            viewport_height: px(120.),
                        })
                    },
                )
            })
            .expect("open a real window");
        cx.run_until_parked();

        let observation = test_observer::take()
            .expect("the composed pane gives the container a bounded height to measure");
        let overlay = observation
            .overlay
            .expect("four real rows overflow a 120px pane shared with a header and a bar");
        assert_eq!(
            observation.content_mask.bounds, observation.viewport,
            "the frame section's content actually painted under the viewport's own mask, i.e. it is clipped"
        );

        let window_bounds = cx
            .update_window(window.into(), |_, window, _| window.viewport_size())
            .expect("read the window's viewport size");

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        let painted_overlay = vcx
            .debug_bounds("overflow-fade-overlay")
            .expect("the overlay actually painted this frame");
        assert_eq!(painted_overlay, overlay);

        let bar_bounds = vcx
            .debug_bounds("bottom-bar")
            .expect("the bar actually painted this frame");
        assert!(
            bar_bounds.size.height > px(0.) && bar_bounds.size.width > px(0.),
            "the bar's own painted bounds are non-zero, not a stubbed-out probe"
        );
        assert!(
            painted_overlay.origin.y + painted_overlay.size.height <= bar_bounds.origin.y,
            "the overlay's painted bottom edge sits at or above the bar's painted top edge"
        );
        assert!(
            bar_bounds.origin.y + bar_bounds.size.height <= window_bounds.height,
            "the bar's painted bottom edge stays within the window it is composed into"
        );
    }
}

//! Two small time charts for the overview (one series each, so the title names it and
//! there is no legend). Columns for a count per minute; a line with a light wash for a
//! level over time. Hover or use the arrow keys for a readout; a table view is included.

use leptos::prelude::*;
use sl_ui::format;

const H: f64 = 132.0;
const PAD_L: f64 = 30.0;
const PAD_B: f64 = 20.0;
const PAD_T: f64 = 8.0;

fn nice_max(v: u32) -> u32 {
    let v = v.max(1);
    for step in [
        1u32, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 5000, 10_000,
    ] {
        if v <= step * 4 {
            return step * v.div_ceil(step);
        }
    }
    v
}

/// Shared frame: measures its width and tracks the hovered/focused index.
fn frame_width(el: NodeRef<leptos::html::Div>, width: RwSignal<f64>) {
    let measure = move || {
        if let Some(d) = el.get_untracked() {
            let w = d.client_width() as f64;
            if w > 0.0 {
                width.set(w);
            }
        }
    };
    request_animation_frame(measure);
    let h = window_event_listener(leptos::ev::resize, move |_| measure());
    on_cleanup(move || h.remove());
}

#[component]
pub fn TimeChart(
    #[prop(into)] title: String,
    /// `(minute start ms, value)` oldest first.
    #[prop(into)]
    points: Signal<Vec<(i64, u32)>>,
    /// "message"/"messages" etc.
    one: &'static str,
    many: &'static str,
    /// Columns (counts) or line (levels).
    columns: bool,
) -> impl IntoView {
    let el: NodeRef<leptos::html::Div> = NodeRef::new();
    let width = RwSignal::new(560.0);
    let active = RwSignal::new(None::<usize>);
    frame_width(el, width);

    let geom = Memo::new(move |_| {
        let pts = points.get();
        let w = width.get();
        let n = pts.len().max(1);
        let max = nice_max(pts.iter().map(|p| p.1).max().unwrap_or(0));
        let plot_w = (w - PAD_L).max(10.0);
        let step = plot_w / n as f64;
        (pts, w, n, max, step)
    });
    let y = move |v: u32, max: u32| PAD_T + (H - PAD_T - PAD_B) * (1.0 - v as f64 / max as f64);

    let readout = move || {
        let (pts, ..) = geom.get();
        let idx = active.get().unwrap_or(pts.len().saturating_sub(1));
        match pts.get(idx) {
            Some((ts, v)) => {
                let what = if *v == 1 { one } else { many };
                if active.get().is_some() {
                    format!("{}: {v} {what}", format::time(*ts))
                } else {
                    format!("Now: {v} {what}")
                }
            }
            None => "No data yet".into(),
        }
    };

    let on_move = move |e: leptos::ev::PointerEvent| {
        let Some(d) = el.get_untracked() else { return };
        let rect = d.get_bounding_client_rect();
        let x = e.client_x() as f64 - rect.left() - PAD_L;
        let (_, _, n, _, step) = geom.get_untracked();
        if x < 0.0 {
            active.set(None);
            return;
        }
        active.set(Some(((x / step) as usize).min(n - 1)));
    };
    let on_key = move |e: leptos::ev::KeyboardEvent| {
        let (_, _, n, _, _) = geom.get_untracked();
        let cur = active.get_untracked().unwrap_or(n - 1);
        let next = match e.key().as_str() {
            "ArrowLeft" => cur.saturating_sub(1),
            "ArrowRight" => (cur + 1).min(n - 1),
            "Home" => 0,
            "End" => n - 1,
            "Escape" => {
                active.set(None);
                return;
            }
            _ => return,
        };
        e.prevent_default();
        active.set(Some(next));
    };

    let svg = move || {
        let (pts, w, n, max, step) = geom.get();
        let base = H - PAD_B;
        let top = y(max, max);
        let mut out: Vec<AnyView> = Vec::new();
        // Recessive frame: baseline and one top gridline with its value.
        out.push(view! { <line class="chart-grid" x1=PAD_L y1=top x2=w y2=top></line> }.into_any());
        out.push(
            view! { <line class="chart-grid" x1=PAD_L y1=base x2=w y2=base></line> }.into_any(),
        );
        out.push(view! { <text class="chart-text" x=PAD_L - 6.0 y=top + 4.0 text-anchor="end">{max.to_string()}</text> }.into_any());
        out.push(view! { <text class="chart-text" x=PAD_L - 6.0 y=base + 4.0 text-anchor="end">"0"</text> }.into_any());
        out.push(
            view! { <text class="chart-text" x=PAD_L y=H - 4.0>"60 min ago"</text> }.into_any(),
        );
        out.push(
            view! { <text class="chart-text" x=w y=H - 4.0 text-anchor="end">"now"</text> }
                .into_any(),
        );
        let act = active.get();
        if columns {
            let bw = (step - 2.0).clamp(1.0, 24.0);
            let r = (bw / 2.0).min(3.0);
            for (i, (_, v)) in pts.iter().enumerate() {
                if *v == 0 {
                    continue;
                }
                let x = PAD_L + i as f64 * step + (step - bw) / 2.0;
                let yt = y(*v, max);
                let h = (base - yt).max(1.0);
                // Rounded top, square at the baseline.
                let d = format!(
                    "M{x},{base} V{} Q{x},{yt} {},{yt} H{} Q{},{yt} {},{} V{base} Z",
                    yt + r,
                    x + r,
                    x + bw - r,
                    x + bw,
                    x + bw,
                    yt + r,
                );
                let d = if h < r * 2.0 {
                    format!("M{x},{base} V{yt} H{} V{base} Z", x + bw)
                } else {
                    d
                };
                let on = act == Some(i);
                out.push(view! { <path class="chart-bar" class:on=on d=d></path> }.into_any());
            }
            let _ = n;
        } else if !pts.is_empty() {
            let xy: Vec<(f64, f64)> = pts
                .iter()
                .enumerate()
                .map(|(i, (_, v))| (PAD_L + (i as f64 + 0.5) * step, y(*v, max)))
                .collect();
            let line: String = xy
                .iter()
                .enumerate()
                .map(|(i, (x, y))| format!("{}{x:.1},{y:.1}", if i == 0 { "M" } else { " L" }))
                .collect();
            let area = format!(
                "{line} L{:.1},{base} L{:.1},{base} Z",
                xy.last().map(|p| p.0).unwrap_or(PAD_L),
                xy[0].0
            );
            out.push(view! { <path class="chart-area" d=area></path> }.into_any());
            out.push(view! { <path class="chart-line" d=line></path> }.into_any());
            let i = act.unwrap_or(xy.len() - 1);
            if let Some((x, yy)) = xy.get(i).copied() {
                if act.is_some() {
                    out.push(
                        view! { <line class="chart-cross" x1=x y1=PAD_T x2=x y2=base></line> }
                            .into_any(),
                    );
                }
                out.push(view! { <circle class="chart-dot" cx=x cy=yy r="4"></circle> }.into_any());
            }
        }
        view! {
            <svg viewBox=format!("0 0 {w} {H}") height=H aria-hidden="true" focusable="false">{out}</svg>
        }
    };

    let tip = move || {
        let i = active.get()?;
        let (pts, _, _, _, step) = geom.get();
        let (ts, v) = pts.get(i).copied()?;
        let left = PAD_L + (i as f64 + 0.5) * step;
        Some(
            view! { <div class="chart-tip" style=format!("left:{left}px")>{format!("{}: {v}", format::time(ts))}</div> },
        )
    };

    let summary = {
        let title = title.clone();
        move || {
            let (pts, ..) = geom.get();
            let total: u64 = pts.iter().map(|p| p.1 as u64).sum();
            let peak = pts.iter().map(|p| p.1).max().unwrap_or(0);
            if columns {
                format!(
                    "{title}. {total} in the last hour, at most {peak} in one minute. Use the arrow keys to read each minute."
                )
            } else {
                let now = pts.last().map(|p| p.1).unwrap_or(0);
                format!(
                    "{title}. {now} now, at most {peak} in the last hour. Use the arrow keys to read each minute."
                )
            }
        }
    };

    let title2 = title.clone();
    view! {
        <div class="chart-card">
            <h3>{title2}</h3>
            <p class="readout" aria-live="polite">{readout}</p>
            <div
                class="chart-wrap"
                node_ref=el
                tabindex="0"
                role="img"
                aria-label=summary
                on:pointermove=on_move
                on:pointerleave=move |_| active.set(None)
                on:keydown=on_key
                on:blur=move |_| active.set(None)
            >
                {svg}
                {tip}
            </div>
            <details>
                <summary>"Show as table"</summary>
                <table class="table" style="margin-top:var(--s2)">
                    <thead><tr><th scope="col">"Minute"</th><th scope="col">{title.clone()}</th></tr></thead>
                    <tbody>
                        {move || points.get().into_iter().rev().filter(|p| p.1 > 0).take(60).map(|(ts, v)| view! {
                            <tr><td>{format::time(ts)}</td><td>{v}</td></tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </details>
        </div>
    }
}

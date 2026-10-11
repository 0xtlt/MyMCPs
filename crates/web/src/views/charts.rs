//! Charts and figures, drawn by the server: the old pages drew them in the
//! browser with chart.js, and the content security policy now leaves no
//! room for the inline styles a chart library needs. Bars take their height
//! from `data-pct`, lines are an inline `<svg>` in data units ("Dynamic
//! values" in `docs/design-system.md`, and the comment above `.line-chart`
//! in `app.css`).

use maud::{Markup, html};

/// A count as people read it: `8,412`.
pub fn number(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        text.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// `1 call`, `2 calls`, `1,204 calls`.
pub fn counted(value: i64, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        number(value),
        if value == 1 { singular } else { plural }
    )
}

/// `Number.prototype.toFixed(1)`: one decimal, the larger neighbour on a
/// tie. Rust rounds a tie to the even digit (`0.25` gives `0.2`), which
/// would show another rate than the old page for the same calls.
pub fn fixed_1(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    // The exact decimal expansion of the double: every digit it has.
    let exact = format!("{:.330}", value.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((exact.as_str(), "0"));
    let mut tenths = fraction[..1].parse::<u32>().unwrap_or(0);
    let mut whole = whole.parse::<u128>().unwrap_or(0);
    if fraction[1..].starts_with(|digit: char| digit >= '5') {
        tenths += 1;
        if tenths == 10 {
            tenths = 0;
            whole += 1;
        }
    }
    let sign = if value < 0.0 && (whole > 0 || tenths > 0) {
        "-"
    } else {
        ""
    };
    format!("{sign}{whole}.{tenths}")
}

/// The top of a vertical axis with four equal intervals: the step is 1, 2,
/// 2.5 or 5 times a power of ten, and always a whole number.
pub fn nice_axis_max(max: i64) -> i64 {
    if max <= 4 {
        return 4;
    }
    // The largest power of ten that is at most a quarter of the maximum.
    let mut power: i64 = 1;
    while power
        .checked_mul(40)
        .is_some_and(|next_quarter| next_quarter <= max)
    {
        power *= 10;
    }
    let candidates = [
        Some(power),
        Some(power.saturating_mul(2)),
        // 2.5 times one is not a whole number.
        (power >= 10).then(|| (power / 2).saturating_mul(5)),
        Some(power.saturating_mul(5)),
        Some(power.saturating_mul(10)),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|step| step.saturating_mul(4) >= max)
        .unwrap_or(i64::MAX)
        .saturating_mul(4)
}

/// A coordinate with at most `digits` decimals and no trailing zero.
fn coordinate(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text.as_str()
    };
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// Where each value is drawn. One value alone sits in the middle of the plot.
fn points(values: &[i64], axis_max: i64) -> Vec<(f64, f64)> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = if values.len() == 1 { 0.5 } else { index as f64 };
            (x, (axis_max - value) as f64)
        })
        .collect()
}

/// The curve through the values: a uniform Catmull-Rom spline written as
/// cubic Béziers, the first and the last point standing in for the
/// neighbour they do not have. The control points are clamped to the plot,
/// so a count never shows below zero or above the axis.
pub fn line_path(values: &[i64], axis_max: i64) -> String {
    let points = points(values, axis_max);
    let Some(first) = points.first() else {
        return String::new();
    };
    let clamp = |y: f64| y.clamp(0.0, axis_max as f64);
    let mut path = format!("M{} {}", coordinate(first.0, 3), coordinate(first.1, 1));
    for index in 0..points.len() - 1 {
        let before = points[index.saturating_sub(1)];
        let from = points[index];
        let to = points[index + 1];
        let after = points[(index + 2).min(points.len() - 1)];
        let first_control = (
            from.0 + (to.0 - before.0) / 6.0,
            clamp(from.1 + (to.1 - before.1) / 6.0),
        );
        let second_control = (
            to.0 - (after.0 - from.0) / 6.0,
            clamp(to.1 - (after.1 - from.1) / 6.0),
        );
        path.push_str(&format!(
            "C{} {} {} {} {} {}",
            coordinate(first_control.0, 3),
            coordinate(first_control.1, 1),
            coordinate(second_control.0, 3),
            coordinate(second_control.1, 1),
            coordinate(to.0, 3),
            coordinate(to.1, 1),
        ));
    }
    path
}

/// The curve closed on the baseline.
pub fn area_path(values: &[i64], axis_max: i64) -> String {
    format!("{}V{axis_max}H0Z", line_path(values, axis_max))
}

/// One zero-length segment for each value: its round caps draw the dot.
pub fn point_paths(values: &[i64], axis_max: i64) -> Vec<String> {
    points(values, axis_max)
        .into_iter()
        .map(|(x, y)| format!("M{} {}h0", coordinate(x, 3), coordinate(y, 1)))
        .collect()
}

/// One period of a line chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePoint {
    pub label: String,
    pub calls: i64,
    pub errors: i64,
}

/// How many labels fit under a line chart.
const MAX_X_LABELS: usize = 7;

/// Calls and errors over time: the y labels, the plot, the x labels and the
/// legend. All text stays HTML, so it never stretches with the plot.
pub fn line_chart(description: &str, timeline: &[LinePoint]) -> Markup {
    let calls: Vec<i64> = timeline.iter().map(|point| point.calls).collect();
    let errors: Vec<i64> = timeline.iter().map(|point| point.errors).collect();
    let highest = calls.iter().chain(&errors).copied().max().unwrap_or(0);
    let axis_max = nice_axis_max(highest);
    let step = axis_max / 4;
    let width = timeline.len().saturating_sub(1).max(1);
    let grid: String = (0..=4)
        .map(|tick| format!("M0 {}H{width}", tick * step))
        .collect();
    // With more periods than labels fit, every k-th keeps its label.
    let label_every = timeline.len().div_ceil(MAX_X_LABELS).max(1);

    html! {
        div class="line-chart" {
            div class="line-chart__y" aria-hidden="true" {
                @for tick in (0..=4).rev() { span { (number(tick * step)) } }
            }
            div class="line-chart__canvas" {
                svg class="line-chart__svg" viewBox=(format!("0 0 {width} {axis_max}")) preserveAspectRatio="none"
                    role="img" aria-label=(description) {
                    path class="line-chart__grid" d=(grid) {}
                    g class="line-chart__series" {
                        @if timeline.len() > 1 {
                            path class="line-chart__area" d=(area_path(&calls, axis_max)) {}
                            path class="line-chart__line" d=(line_path(&calls, axis_max)) {}
                        }
                        @for point in point_paths(&calls, axis_max) { path class="line-chart__point" d=(point) {} }
                    }
                    g class="line-chart__series line-chart__series--critical" {
                        @if timeline.len() > 1 {
                            path class="line-chart__line" d=(line_path(&errors, axis_max)) {}
                        }
                        @for point in point_paths(&errors, axis_max) { path class="line-chart__point" d=(point) {} }
                    }
                    @for (index, point) in timeline.iter().enumerate() {
                        @let x = if timeline.len() == 1 { 0.0 } else { index as f64 - 0.5 };
                        rect class="line-chart__hit" x=(coordinate(x, 1)) y="0" width="1" height=(axis_max) {
                            title {
                                (point.label) ": " (counted(point.calls, "call", "calls")) ", "
                                (counted(point.errors, "error", "errors"))
                            }
                        }
                    }
                }
            }
            div class="line-chart__x" aria-hidden="true" {
                @for (index, point) in timeline.iter().enumerate() {
                    span { @if index % label_every == 0 { (point.label) } }
                }
            }
            ul class="line-chart__legend" {
                li class="line-chart__key" { "Calls" }
                li class="line-chart__key line-chart__key--critical" { "Errors" }
            }
        }
        // The same numbers for people who do not see the chart.
        table class="visually-hidden" {
            caption { (description) }
            thead { tr { th scope="col" { "Period" } th scope="col" { "Calls" } th scope="col" { "Errors" } } }
            tbody {
                @for point in timeline {
                    tr { th scope="row" { (point.label) } td { (number(point.calls)) } td { (number(point.errors)) } }
                }
            }
        }
    }
}

/// One bar of a bar chart: what it stands for, and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub label: String,
    pub value: i64,
}

/// The height of each bar, as a whole share of the tallest one.
pub fn bar_shares(values: &[i64]) -> Vec<i64> {
    let highest = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|value| {
            if highest <= 0 {
                0
            } else {
                // Rounded to the nearest whole percent.
                ((value.max(&0) * 200 + highest) / (highest * 2)).min(100)
            }
        })
        .collect()
}

/// One bar for each period, the last one being the current period.
pub fn bar_chart(description: &str, unit: (&str, &str), bars: &[Bar]) -> Markup {
    let values: Vec<i64> = bars.iter().map(|bar| bar.value).collect();
    html! {
        div class="bar-chart" role="img" aria-label=(description) {
            @for (bar, share) in bars.iter().zip(bar_shares(&values)) {
                span class="bar-chart__bar" data-pct=(share)
                    title=(format!("{}: {}", bar.label, counted(bar.value, unit.0, unit.1))) {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_counts_with_thousands_separators() {
        for (value, text) in [
            (0, "0"),
            (7, "7"),
            (999, "999"),
            (1_000, "1,000"),
            (8_412, "8,412"),
            (12_345_678, "12,345,678"),
            (-1_204, "-1,204"),
        ] {
            assert_eq!(number(value), text);
        }
        assert_eq!(counted(1, "call", "calls"), "1 call");
        assert_eq!(counted(1_204, "call", "calls"), "1,204 calls");
        assert_eq!(counted(0, "error", "errors"), "0 errors");
    }

    #[test]
    fn rounds_rates_as_javascript_does() {
        // What `(value).toFixed(1)` answers in Node.
        for (value, text) in [
            (0.0, "0.0"),
            (50.0, "50.0"),
            (98.6, "98.6"),
            (0.25, "0.3"),
            (12.25, "12.3"),
            (0.75, "0.8"),
            (0.05, "0.1"),
            (99.95, "100.0"),
            (99.94, "99.9"),
            (33.333333333333336, "33.3"),
            (66.66666666666667, "66.7"),
            (100.0, "100.0"),
            (1.45, "1.4"),
            (8.345, "8.3"),
            (0.35, "0.3"),
            (0.15, "0.1"),
            (2.5, "2.5"),
        ] {
            assert_eq!(fixed_1(value), text, "{value}");
        }
    }

    #[test]
    fn picks_an_axis_of_four_whole_steps() {
        // The answers of `niceAxisMax` in the design's reference script.
        for (max, axis) in [
            (0, 4),
            (3, 4),
            (4, 4),
            (5, 8),
            (7, 8),
            (8, 8),
            (9, 20),
            (12, 20),
            (20, 20),
            (21, 40),
            (40, 40),
            (41, 80),
            (99, 100),
            (100, 100),
            (101, 200),
            (480, 800),
            (1_000, 1_000),
            (1_907, 2_000),
            (2_001, 4_000),
            (12_500, 20_000),
            (99_999, 100_000),
            (250_000, 400_000),
            (400_001, 800_000),
        ] {
            assert_eq!(nice_axis_max(max), axis, "{max}");
        }
        assert_eq!(nice_axis_max(i64::MAX), i64::MAX);
    }

    #[test]
    fn draws_the_curve_of_the_reference_script() {
        let calls = [980, 1240, 1105, 720, 640, 1820, 1907];
        let errors = [12, 18, 9, 31, 6, 22, 20];
        assert_eq!(
            line_path(&calls, 2000),
            "M0 1020C0.167 976.7 0.667 780.8 1 760C1.333 739.2 1.667 808.3 2 895C2.333 981.7 2.667 1202.5 3 1280C3.333 1357.5 3.667 1543.3 4 1360C4.333 1176.7 4.667 391.2 5 180C5.333 0 5.833 107.5 6 93"
        );
        assert_eq!(
            line_path(&errors, 2000),
            "M0 1988C0.167 1987 0.667 1981.5 1 1982C1.333 1982.5 1.667 1993.2 2 1991C2.333 1988.8 2.667 1968.5 3 1969C3.333 1969.5 3.667 1992.5 4 1994C4.333 1995.5 4.667 1980.3 5 1978C5.333 1975.7 5.833 1979.7 6 1980"
        );
        assert!(area_path(&calls, 2000).ends_with(" 6 93V2000H0Z"));
        assert_eq!(
            point_paths(&calls, 2000),
            [
                "M0 1020h0",
                "M1 760h0",
                "M2 895h0",
                "M3 1280h0",
                "M4 1360h0",
                "M5 180h0",
                "M6 93h0"
            ]
        );
    }

    #[test]
    fn draws_the_whole_line_chart() {
        let labels = [
            "Oct 1", "Oct 2", "Oct 3", "Oct 4", "Oct 5", "Oct 6", "Oct 7",
        ];
        let calls = [980, 1240, 1105, 720, 640, 1820, 1907];
        let errors = [12, 18, 9, 31, 6, 22, 20];
        let timeline: Vec<LinePoint> = labels
            .iter()
            .zip(calls.iter().zip(errors))
            .map(|(label, (calls, errors))| LinePoint {
                label: label.to_string(),
                calls: *calls,
                errors,
            })
            .collect();
        let chart =
            line_chart("Calls and errors over the selected time range", &timeline).into_string();

        assert!(chart.contains("viewBox=\"0 0 6 2000\""));
        assert!(chart.contains("d=\"M0 0H6M0 500H6M0 1000H6M0 1500H6M0 2000H6\""));
        assert!(chart.contains(
            "<span>2,000</span><span>1,500</span><span>1,000</span><span>500</span><span>0</span>"
        ));
        assert!(chart.contains(
            "<rect class=\"line-chart__hit\" x=\"-0.5\" y=\"0\" width=\"1\" height=\"2000\"><title>Oct 1: 980 calls, 12 errors</title></rect>"
        ));
        assert!(chart.contains("<title>Oct 7: 1,907 calls, 20 errors</title>"));
        assert!(chart.contains("<span>Oct 1</span><span>Oct 2</span>"));
        assert!(chart.contains("<tr><th scope=\"row\">Oct 2</th><td>1,240</td><td>18</td></tr>"));
        // No inline style: the policy would drop it.
        assert!(!chart.contains("style="));
    }

    #[test]
    fn keeps_at_most_seven_labels_and_centres_a_single_period() {
        let timeline: Vec<LinePoint> = (0..24)
            .map(|hour| LinePoint {
                label: format!("{hour:02}:00"),
                calls: hour,
                errors: 0,
            })
            .collect();
        let chart = line_chart("Calls", &timeline).into_string();
        let labels = chart.matches(":00</span>").count();
        assert_eq!(labels, 6);
        assert!(chart.contains(
            "<span>00:00</span><span></span><span></span><span></span><span>04:00</span>"
        ));

        let single = line_chart(
            "Calls",
            &[LinePoint {
                label: "01:30".into(),
                calls: 1,
                errors: 1,
            }],
        )
        .into_string();
        assert!(single.contains("viewBox=\"0 0 1 4\""));
        assert!(single.contains("d=\"M0.5 3h0\""));
        assert!(!single.contains("line-chart__line"));
        assert!(single.contains(
            "x=\"0\" y=\"0\" width=\"1\" height=\"4\"><title>01:30: 1 call, 1 error</title>"
        ));
    }

    #[test]
    fn sizes_bars_against_the_tallest() {
        assert_eq!(bar_shares(&[0, 0, 0]), [0, 0, 0]);
        assert_eq!(bar_shares(&[5, 10, 0, 1]), [50, 100, 0, 10]);
        assert_eq!(bar_shares(&[1, 3]), [33, 100]);
        assert_eq!(bar_shares(&[1, 200]), [1, 100]);
        let chart = bar_chart(
            "Tool calls per day, last 14 days",
            ("call", "calls"),
            &[
                Bar {
                    label: "6 Oct".into(),
                    value: 1,
                },
                Bar {
                    label: "7 Oct".into(),
                    value: 4,
                },
            ],
        )
        .into_string();
        assert_eq!(
            chart,
            "<div class=\"bar-chart\" role=\"img\" aria-label=\"Tool calls per day, last 14 days\"><span class=\"bar-chart__bar\" data-pct=\"25\" title=\"6 Oct: 1 call\"></span><span class=\"bar-chart__bar\" data-pct=\"100\" title=\"7 Oct: 4 calls\"></span></div>"
        );
    }
}

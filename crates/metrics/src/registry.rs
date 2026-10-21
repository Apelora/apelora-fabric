// SPDX-License-Identifier: AGPL-3.0-only
//! Counters, gauges and histograms with a fixed set of label names, and their rendering in the
//! text exposition format (and its OpenMetrics variant).
//!
//! Cardinality is bounded twice: callers pass only low-cardinality values (route templates,
//! outcome classes, catalogue ids — see `docs/architecture/MONITORING.md`), and each family
//! keeps at most [`MAX_SERIES`] label sets. Anything beyond that is counted under one series
//! whose labels are all [`OVERFLOW`], so a bug that leaks an id into a label shows up as a
//! visible overflow series instead of unbounded memory.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard};

/// Most label sets one family keeps.
pub const MAX_SERIES: usize = 1000;
/// The label value used for every label of the overflow series.
pub const OVERFLOW: &str = "_overflow";
/// Label values are cut to this many characters.
pub const MAX_VALUE_LEN: usize = 96;

type Key = Vec<String>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock leaves plain numbers behind: still safe to read.
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One label value as stored: no control characters, bounded length.
fn clean(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_control())
        .take(MAX_VALUE_LEN)
        .collect()
}

/// The key for `values`, or the overflow key when the family is full and this label set is new.
/// A call with the wrong number of values is a programming error: padded or cut, never a panic.
fn slot<V>(series: &BTreeMap<Key, V>, names: &[&str], values: &[&str]) -> Key {
    let key: Key = (0..names.len())
        .map(|i| clean(values.get(i).copied().unwrap_or("")))
        .collect();
    if series.contains_key(&key) || series.len() < MAX_SERIES {
        key
    } else {
        vec![OVERFLOW.to_owned(); names.len()]
    }
}

/// What every family can do: render itself.
pub trait Family: Sync {
    fn name(&self) -> &'static str;
    fn label_names(&self) -> &'static [&'static str];
    fn render(&self, out: &mut String, constant: &[(String, String)], openmetrics: bool);
}

fn escape_label(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '\n' => s.push_str("\\n"),
            c => s.push(c),
        }
    }
    s
}

fn escape_help(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n")
}

/// A number in exposition syntax.
pub fn number(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v == f64::INFINITY {
        "+Inf".into()
    } else if v == f64::NEG_INFINITY {
        "-Inf".into()
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn labels(
    constant: &[(String, String)],
    names: &[&str],
    values: &[String],
    extra: Option<(&str, &str)>,
) -> String {
    let mut parts: Vec<String> = constant
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape_label(v)))
        .collect();
    parts.extend(
        names
            .iter()
            .zip(values)
            .map(|(k, v)| format!("{k}=\"{}\"", escape_label(v))),
    );
    if let Some((k, v)) = extra {
        parts.push(format!("{k}=\"{}\"", escape_label(v)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", parts.join(","))
    }
}

fn header(out: &mut String, name: &str, help: &str, kind: &str) {
    let _ = writeln!(out, "# HELP {name} {}", escape_help(help));
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

/// A monotonically increasing count. `name` is given without the `_total` suffix.
pub struct Counter {
    name: &'static str,
    help: &'static str,
    labels: &'static [&'static str],
    series: Mutex<BTreeMap<Key, f64>>,
}

impl Counter {
    pub const fn new(
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            help,
            labels,
            series: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn inc(&self, values: &[&str]) {
        self.add(values, 1.0);
    }

    pub fn add(&self, values: &[&str], v: f64) {
        if !v.is_finite() || v < 0.0 {
            return;
        }
        let mut s = lock(&self.series);
        let k = slot(&s, self.labels, values);
        *s.entry(k).or_insert(0.0) += v;
    }

    /// The current value (tests and the Console summary).
    pub fn get(&self, values: &[&str]) -> f64 {
        let k: Key = values.iter().map(|v| clean(v)).collect();
        lock(&self.series).get(&k).copied().unwrap_or(0.0)
    }
}

impl Family for Counter {
    fn name(&self) -> &'static str {
        self.name
    }
    fn label_names(&self) -> &'static [&'static str] {
        self.labels
    }
    fn render(&self, out: &mut String, constant: &[(String, String)], openmetrics: bool) {
        let sample = format!("{}_total", self.name);
        header(
            out,
            if openmetrics { self.name } else { &sample },
            self.help,
            "counter",
        );
        for (k, v) in lock(&self.series).iter() {
            let _ = writeln!(
                out,
                "{sample}{} {}",
                labels(constant, self.labels, k, None),
                number(*v)
            );
        }
    }
}

/// A value that goes up and down.
pub struct Gauge {
    name: &'static str,
    help: &'static str,
    labels: &'static [&'static str],
    series: Mutex<BTreeMap<Key, f64>>,
}

impl Gauge {
    pub const fn new(
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            help,
            labels,
            series: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn set(&self, values: &[&str], v: f64) {
        let mut s = lock(&self.series);
        let k = slot(&s, self.labels, values);
        s.insert(k, v);
    }

    pub fn add(&self, values: &[&str], v: f64) {
        let mut s = lock(&self.series);
        let k = slot(&s, self.labels, values);
        *s.entry(k).or_insert(0.0) += v;
    }

    /// Replaces every series at once (a state read in full: members by status, jobs by kind),
    /// so a state that no longer occurs disappears instead of keeping its last value.
    pub fn replace<'a>(&self, rows: impl IntoIterator<Item = (Vec<&'a str>, f64)>) {
        let mut s = lock(&self.series);
        s.clear();
        for (values, v) in rows {
            let k = slot(&s, self.labels, &values);
            *s.entry(k).or_insert(0.0) += v;
        }
    }

    pub fn get(&self, values: &[&str]) -> Option<f64> {
        let k: Key = values.iter().map(|v| clean(v)).collect();
        lock(&self.series).get(&k).copied()
    }
}

impl Family for Gauge {
    fn name(&self) -> &'static str {
        self.name
    }
    fn label_names(&self) -> &'static [&'static str] {
        self.labels
    }
    fn render(&self, out: &mut String, constant: &[(String, String)], _openmetrics: bool) {
        header(out, self.name, self.help, "gauge");
        for (k, v) in lock(&self.series).iter() {
            let _ = writeln!(
                out,
                "{}{} {}",
                self.name,
                labels(constant, self.labels, k, None),
                number(*v)
            );
        }
    }
}

#[derive(Clone)]
struct Buckets {
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

/// Observations counted into fixed buckets (upper bounds in seconds or bytes).
pub struct Histogram {
    name: &'static str,
    help: &'static str,
    labels: &'static [&'static str],
    bounds: &'static [f64],
    series: Mutex<BTreeMap<Key, Buckets>>,
}

impl Histogram {
    pub const fn new(
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
        bounds: &'static [f64],
    ) -> Self {
        Self {
            name,
            help,
            labels,
            bounds,
            series: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn observe(&self, values: &[&str], v: f64) {
        if !v.is_finite() {
            return;
        }
        let mut s = lock(&self.series);
        let k = slot(&s, self.labels, values);
        let b = s.entry(k).or_insert_with(|| Buckets {
            counts: vec![0; self.bounds.len()],
            sum: 0.0,
            count: 0,
        });
        if let Some(i) = self.bounds.iter().position(|le| v <= *le) {
            b.counts[i] += 1;
        }
        b.sum += v;
        b.count += 1;
    }

    /// How many observations a label set has (tests).
    pub fn count(&self, values: &[&str]) -> u64 {
        let k: Key = values.iter().map(|v| clean(v)).collect();
        lock(&self.series).get(&k).map_or(0, |b| b.count)
    }
}

impl Family for Histogram {
    fn name(&self) -> &'static str {
        self.name
    }
    fn label_names(&self) -> &'static [&'static str] {
        self.labels
    }
    fn render(&self, out: &mut String, constant: &[(String, String)], _openmetrics: bool) {
        header(out, self.name, self.help, "histogram");
        for (k, b) in lock(&self.series).iter() {
            let mut cumulative = 0;
            for (le, n) in self.bounds.iter().zip(&b.counts) {
                cumulative += n;
                let _ = writeln!(
                    out,
                    "{}_bucket{} {cumulative}",
                    self.name,
                    labels(constant, self.labels, k, Some(("le", &number(*le))))
                );
            }
            let _ = writeln!(
                out,
                "{}_bucket{} {}",
                self.name,
                labels(constant, self.labels, k, Some(("le", "+Inf"))),
                b.count
            );
            let l = labels(constant, self.labels, k, None);
            let _ = writeln!(out, "{}_sum{l} {}", self.name, number(b.sum));
            let _ = writeln!(out, "{}_count{l} {}", self.name, b.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_render_with_the_total_suffix_in_both_formats() {
        let c = Counter::new("t_requests", "Requests.", &["route"]);
        c.inc(&["/a"]);
        c.add(&["/a"], 2.0);
        c.add(&["/a"], -5.0); // ignored: counters never go down
        let mut text = String::new();
        c.render(&mut text, &[], false);
        assert!(text.contains("# TYPE t_requests_total counter"), "{text}");
        assert!(text.contains("t_requests_total{route=\"/a\"} 3"), "{text}");
        let mut om = String::new();
        c.render(&mut om, &[], true);
        assert!(om.contains("# TYPE t_requests counter"), "{om}");
        assert!(om.contains("t_requests_total{route=\"/a\"} 3"), "{om}");
    }

    #[test]
    fn constant_labels_come_first_and_values_are_escaped() {
        let g = Gauge::new("t_g", "G.", &["x"]);
        g.set(&["a\"b\\c\nd"], 1.5);
        let mut out = String::new();
        g.render(&mut out, &[("region".into(), "eu".into())], false);
        // Control characters are dropped when stored; quotes and backslashes are escaped.
        assert!(
            out.contains("t_g{region=\"eu\",x=\"a\\\"b\\\\cd\"} 1.5"),
            "{out}"
        );
    }

    #[test]
    fn a_family_keeps_at_most_max_series_and_counts_the_rest_as_overflow() {
        let c = Counter::new("t_many", "Many.", &["id"]);
        for i in 0..(MAX_SERIES + 50) {
            c.inc(&[&format!("id-{i}")]);
        }
        assert_eq!(c.get(&[OVERFLOW]), 50.0);
        assert_eq!(lock(&c.series).len(), MAX_SERIES + 1);
        // A label set seen before the limit still counts as itself.
        c.inc(&["id-0"]);
        assert_eq!(c.get(&["id-0"]), 2.0);
    }

    #[test]
    fn histograms_are_cumulative_with_sum_and_count() {
        let h = Histogram::new("t_h", "H.", &[], &[0.1, 1.0]);
        h.observe(&[], 0.05);
        h.observe(&[], 0.5);
        h.observe(&[], 5.0);
        let mut out = String::new();
        h.render(&mut out, &[], false);
        assert!(out.contains("t_h_bucket{le=\"0.1\"} 1"), "{out}");
        assert!(out.contains("t_h_bucket{le=\"1\"} 2"), "{out}");
        assert!(out.contains("t_h_bucket{le=\"+Inf\"} 3"), "{out}");
        assert!(out.contains("t_h_sum 5.55"), "{out}");
        assert!(out.contains("t_h_count 3"), "{out}");
    }

    #[test]
    fn replace_drops_states_that_no_longer_occur_and_long_values_are_cut() {
        let g = Gauge::new("t_r", "R.", &["status"]);
        g.replace([(vec!["Online"], 2.0), (vec!["Offline"], 1.0)]);
        g.replace([(vec!["Online"], 3.0)]);
        assert_eq!(g.get(&["Online"]), Some(3.0));
        assert_eq!(g.get(&["Offline"]), None);
        let long = "x".repeat(500);
        g.set(&[&long], 1.0);
        assert!(lock(&g.series).keys().all(|k| k[0].len() <= MAX_VALUE_LEN));
    }

    #[test]
    fn numbers_use_exposition_syntax() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(f64::INFINITY), "+Inf");
        assert_eq!(number(f64::NAN), "NaN");
    }
}

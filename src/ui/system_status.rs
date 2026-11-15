//! Compact system usage: one minute of CPU history and current memory usage.

use crate::sysstat::{Sample, Sampler};
use gtk4::prelude::*;
use gtk4::{cairo, glib, Orientation};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

const INTERVAL: u32 = 2;
const HISTORY_LEN: usize = 30;

struct Strip {
    cpu: gtk4::Label,
    chart: gtk4::DrawingArea,
    ram: gtk4::Label,
    bar: gtk4::ProgressBar,
    capacity: gtk4::Label,
    history: Rc<RefCell<VecDeque<Option<f64>>>>,
}

pub(super) fn new() -> gtk4::Box {
    let (root, strip) = Strip::new();
    let mut sampler = Sampler::default();
    strip.update(sampler.sample_values());
    let weak = root.downgrade();
    glib::timeout_add_seconds_local(INTERVAL, move || {
        let Some(_root) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        strip.update(sampler.sample_values());
        glib::ControlFlow::Continue
    });
    root
}

impl Strip {
    fn new() -> (gtk4::Box, Self) {
        let root = gtk4::Box::new(Orientation::Horizontal, 18);
        root.add_css_class("system-status");
        root.set_valign(gtk4::Align::Center);
        let cpu_group = gtk4::Box::new(Orientation::Horizontal, 6);
        let cpu = value("—", "system-cpu-value");
        cpu.set_width_chars(4);
        let chart = gtk4::DrawingArea::builder().content_width(76).content_height(22).build();
        chart.add_css_class("system-cpu-history");
        chart.update_property(&[gtk4::accessible::Property::Label(
            "CPU usage over the last minute, 0 to 100 percent",
        )]);
        chart.set_tooltip_text(Some("CPU usage · last 60 seconds · 0–100%"));
        let history = Rc::new(RefCell::new(VecDeque::with_capacity(HISTORY_LEN)));
        let samples = history.clone();
        chart.set_draw_func(move |area, cr, w, h| {
            draw_history(cr, &samples.borrow(), area.color(), w as f64, h as f64);
        });
        cpu_group.append(&caption("CPU"));
        cpu_group.append(&chart);
        cpu_group.append(&cpu);

        let memory_group = gtk4::Box::new(Orientation::Horizontal, 6);
        let bar = gtk4::ProgressBar::new();
        bar.add_css_class("system-memory-bar");
        bar.set_size_request(52, -1);
        bar.set_valign(gtk4::Align::Center);
        bar.update_property(&[gtk4::accessible::Property::Label("System memory used")]);
        let ram = value("—", "system-memory-value");
        ram.set_width_chars(4);
        let capacity = caption("");
        capacity.add_css_class("system-memory-capacity");
        memory_group.append(&caption("RAM"));
        memory_group.append(&bar);
        memory_group.append(&ram);
        memory_group.append(&capacity);
        root.append(&cpu_group);
        root.append(&memory_group);
        (
            root,
            Self {
                cpu,
                chart,
                ram,
                bar,
                capacity,
                history,
            },
        )
    }

    fn update(&self, sample: Sample) {
        let cpu = sample.cpu.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 100.0));
        self.cpu.set_label(&cpu.map_or_else(|| "—".into(), |v| format!("{v:.0}%")));
        {
            let mut history = self.history.borrow_mut();
            if history.len() == HISTORY_LEN {
                history.pop_front();
            }
            history.push_back(cpu);
        }
        self.chart.queue_draw();
        let fraction = sample.memory_fraction();
        self.ram
            .set_label(&fraction.map_or_else(|| "—".into(), |v| format!("{:.0}%", v * 100.0)));
        self.bar.set_fraction(fraction.unwrap_or(0.0));
        // An unavailable reading must not look like an empty memory meter.
        self.bar.set_visible(fraction.is_some());
        self.capacity.set_label(&sample.memory_capacity().unwrap_or_default());
        self.bar.update_property(&[gtk4::accessible::Property::ValueText(
            &fraction.map_or_else(|| "Unavailable".into(), |v| format!("{:.0}% used", v * 100.0)),
        )]);
    }
}

fn caption(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("system-metric-label");
    label
}

fn value(text: &str, class: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("system-metric-value");
    label.add_css_class("numeric");
    label.add_css_class(class);
    label.set_xalign(1.0);
    label
}

fn draw_history(cr: &cairo::Context, history: &VecDeque<Option<f64>>, color: gtk4::gdk::RGBA, width: f64, height: f64) {
    let (left, bottom) = (2.0, height - 2.0);
    let dx = (width - 4.0).max(0.0) / (HISTORY_LEN - 1) as f64;
    let dy = (height - 4.0).max(0.0) / 100.0;
    let x = |index: usize| left + (HISTORY_LEN - history.len() + index) as f64 * dx;
    let y = |percent: f64| bottom - percent * dy;
    let set_color = |alpha| cr.set_source_rgba(color.red() as f64, color.green() as f64, color.blue() as f64, alpha);
    let samples: Vec<_> = history.iter().copied().enumerate().collect();
    // Gaps represent missing samples. Do not draw a line through unavailable data.
    for run in samples.split(|(_, value)| value.is_none()).filter(|run| !run.is_empty()) {
        let (first, last) = (run[0].0, run[run.len() - 1].0);
        cr.move_to(x(first), bottom);
        for &(i, value) in run {
            cr.line_to(x(i), y(value.unwrap()));
        }
        cr.line_to(x(last), bottom);
        cr.close_path();
        set_color(0.12);
        let _ = cr.fill();
        for (n, &(i, value)) in run.iter().enumerate() {
            if n == 0 {
                cr.move_to(x(i), y(value.unwrap()));
            } else {
                cr.line_to(x(i), y(value.unwrap()));
            }
        }
        set_color(1.0);
        cr.set_line_width(1.5);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.set_line_cap(cairo::LineCap::Round);
        let _ = cr.stroke();
        // Keep the last point visible, including a lone zero-load sample.
        cr.arc(x(last), y(run[run.len() - 1].1.unwrap()), 1.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_display() -> bool {
        let found = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if !found {
            eprintln!("skipped GTK system status test: no display; run under xvfb-run");
        }
        found
    }

    #[test]
    fn system_status_graph_preserves_zero_full_load_and_missing_samples() {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 80, 24).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        let mut history = VecDeque::from(vec![None; HISTORY_LEN]);
        history[0] = Some(100.0);
        history[HISTORY_LEN - 1] = Some(0.0);
        draw_history(&cr, &history, gtk4::gdk::RGBA::new(0.0, 1.0, 1.0, 1.0), 80.0, 24.0);
        drop(cr);
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        let pixel = |x: usize, y: usize| u32::from_ne_bytes(data[y * stride + x * 4..y * stride + x * 4 + 4].try_into().unwrap());
        assert_ne!(pixel(2, 2) >> 24, 0, "100% belongs at the top");
        assert_ne!(pixel(78, 22) >> 24, 0, "0% is a real sample at the bottom");
        assert_eq!(pixel(40, 12) >> 24, 0, "missing samples must not be connected");
    }

    #[test]
    fn system_status_timer_does_not_retain_the_widget() {
        if !has_display() {
            return;
        }
        gtk4::test_synced(|| {
            let root = new();
            let weak = root.downgrade();
            drop(root);
            assert!(weak.upgrade().is_none());
        });
    }

    #[test]
    fn system_status_retains_one_minute_and_preserves_unavailable_readings() {
        if !has_display() {
            return;
        }
        gtk4::test_synced(|| {
            let (_root, strip) = Strip::new();
            for cpu in 0..90 {
                strip.update(Sample {
                    cpu: Some(cpu as f64),
                    mem: Some((4 * 1024 * 1024, 16 * 1024 * 1024)),
                });
            }
            assert_eq!(strip.history.borrow().len(), HISTORY_LEN);
            assert_eq!(strip.history.borrow().front(), Some(&Some(60.0)));
            assert_eq!(strip.cpu.text(), "89%");
            assert_eq!(strip.ram.text(), "25%");
            assert_eq!(strip.bar.fraction(), 0.25);
            assert_eq!(strip.capacity.text(), "4.0/16.0 GiB");
            assert!(strip.bar.get_visible());

            strip.update(Sample::default());
            assert_eq!(strip.history.borrow().back(), Some(&None));
            assert_eq!(strip.cpu.text(), "—");
            assert_eq!(strip.ram.text(), "—");
            assert_eq!(strip.capacity.text(), "");
            assert!(!strip.bar.get_visible());
            strip.update(Sample {
                cpu: Some(0.0),
                mem: Some((0, 100)),
            });
            assert_eq!(strip.cpu.text(), "0%");
            assert_eq!(strip.ram.text(), "0%");
            assert!(strip.bar.get_visible());
            assert_eq!(strip.history.borrow().back(), Some(&Some(0.0)));
        });
    }
}

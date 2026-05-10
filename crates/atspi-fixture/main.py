#!/usr/bin/env python3
# agent-ctrl AT-SPI fixture - a tiny GTK4 app exposing stable, common widgets
# for testing the `surface-atspi` (Linux) implementation. The AT-SPI analog of
# `agent-ctrl-uia-fixture` (Windows) / `agent-ctrl-ax-fixture` (macOS): tests
# drive *this* rather than a distro app whose UI varies by version/desktop.
#
# Not a Cargo crate (a GTK-rs dependency would bloat the Rust workspace); it
# runs in the docker/linux-dev container, which has GTK4 + the AT-SPI stack.
#
# Usage:
#   python3 crates/atspi-fixture/main.py [--ready-file PATH] [--auto-close-ms MS]
#
# `--ready-file`: written (empty) once the window is mapped, so a test can wait
# for readiness without sleeping. `--auto-close-ms`: quit after this long, so a
# crashed test can't leave the window around forever.
import argparse
import sys

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, GLib  # noqa: E402

# The AT-SPI application name defaults to the program name, which would be
# "python3" here; set it so tests can pin via `--target-process`.
GLib.set_prgname("agent-ctrl-atspi-fixture")
GLib.set_application_name("agent-ctrl AT-SPI Fixture")

APP_ID = "dev.agentctrl.AtspiFixture"
TITLE = "agent-ctrl AT-SPI Fixture"


def set_label(widget, text):
    """Set a widget's accessible name (label), tolerating older GTK4 where the
    enum/binding shape differs."""
    try:
        widget.update_property([Gtk.AccessibleProperty.LABEL], [text])
    except Exception:  # pragma: no cover - best effort
        pass


class FixtureApp(Gtk.Application):
    def __init__(self, ready_file=None, auto_close_ms=None):
        super().__init__(application_id=APP_ID)
        self._ready_file = ready_file
        self._auto_close_ms = auto_close_ms
        self._count = 0
        self.connect("activate", self._on_activate)

    def _on_activate(self, _app):
        win = Gtk.ApplicationWindow(application=self, title=TITLE)
        win.set_default_size(440, 380)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        for m in ("margin_top", "margin_bottom", "margin_start", "margin_end"):
            box.set_property(m, 12)
        win.set_child(box)

        self._status = Gtk.Label(label="Status: idle", xalign=0.0)
        box.append(self._status)

        entry = Gtk.Entry()
        entry.set_text("fixture text")
        set_label(entry, "Message")
        box.append(entry)
        self._entry = entry

        check = Gtk.CheckButton(label="Enable advanced mode")
        box.append(check)

        dropdown = Gtk.DropDown.new_from_strings(["Alpha", "Beta", "Gamma"])
        set_label(dropdown, "Choice")
        box.append(dropdown)

        listbox = Gtk.ListBox()
        set_label(listbox, "Items")
        for name in ("First", "Second", "Third"):
            row = Gtk.ListBoxRow()
            row.set_child(Gtk.Label(label=name, xalign=0.0))
            set_label(row, name)  # so the row's accessible name is its text
            listbox.append(row)
        box.append(listbox)

        incr = Gtk.Button(label="Increment")
        incr.connect("clicked", self._on_increment)
        box.append(incr)

        opener = Gtk.Button(label="Open dialog")
        opener.connect("clicked", lambda _b: self._open_dialog(win))
        box.append(opener)

        win.connect("map", self._on_mapped)
        win.present()

        if self._auto_close_ms:
            GLib.timeout_add(self._auto_close_ms, self._quit)

    def _on_mapped(self, _win):
        if self._ready_file:
            try:
                open(self._ready_file, "w").close()
            except OSError as e:  # pragma: no cover
                print(f"fixture: could not write ready file: {e}", file=sys.stderr)

    def _on_increment(self, _btn):
        self._count += 1
        self._status.set_text(f"Status: count {self._count}")

    def _open_dialog(self, parent):
        dlg = Gtk.Window(transient_for=parent, modal=True, title="Fixture Dialog")
        dlg.set_default_size(240, 120)
        b = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        for m in ("margin_top", "margin_bottom", "margin_start", "margin_end"):
            b.set_property(m, 12)
        b.append(Gtk.Label(label="Fixture dialog body"))
        ok = Gtk.Button(label="Dialog OK")
        ok.connect("clicked", lambda _b: dlg.close())
        b.append(ok)
        dlg.set_child(b)
        dlg.present()

    def _quit(self):
        self.quit()
        return GLib.SOURCE_REMOVE


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--ready-file")
    ap.add_argument("--auto-close-ms", type=int)
    args = ap.parse_args(argv[1:])
    app = FixtureApp(ready_file=args.ready_file, auto_close_ms=args.auto_close_ms)
    return app.run([])


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

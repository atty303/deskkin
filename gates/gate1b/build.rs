// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    slint_build::compile("ui/gate.slint").expect("the approved Gate 1B UI must compile");
}

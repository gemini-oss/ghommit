use std::fmt::Debug;

use colored::Colorize;

pub fn print_intent<T: Debug>(title: &str, debuggable: &T) {
    eprintln!("{}: {:?}", title.bold(), debuggable);
}

pub fn print_intent_plain(title: &str) {
    eprintln!("{}", title.bold());
}

pub fn print_success_and_return<T: Debug>(title: &str, debuggable: T) -> T {
    let s = format!("{}: {:?}", title.bold(), debuggable);

    eprintln!("{}", s.green());

    debuggable
}

pub fn print_success_plain(title: &str) {
    eprintln!("{}", title.bold().green());
}

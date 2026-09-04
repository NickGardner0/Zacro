fn main() {
    if let Err(error) = macro_app::run_macro_visual_test_runner() {
        eprintln!("Macro visual test runner failed: {error:#}");
        std::process::exit(1);
    }
}

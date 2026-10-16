//#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), anyhow::Error> {
    //#[allow(unused_must_use)]
    dpr_lib::run()
}

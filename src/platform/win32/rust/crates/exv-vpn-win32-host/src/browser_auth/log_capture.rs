//! 测试专用日志捕获；按当前线程隔离并行模块测试，测试不跨 runtime 线程。
use std::cell::RefCell;
use std::sync::Once;
thread_local! { static RECORDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }
struct Capture;
impl log::Log for Capture {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target() == "exv.atrust_auth"
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            RECORDS.with(|records| records.borrow_mut().push(r.args().to_string()));
        }
    }
    fn flush(&self) {}
}
static CAPTURE: Capture = Capture;
pub(crate) fn start() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&CAPTURE).expect("认证日志测试 logger");
        log::set_max_level(log::LevelFilter::Info);
    });
    RECORDS.with(|records| records.borrow_mut().clear());
}
pub(crate) fn take() -> Vec<String> {
    RECORDS.with(|records| std::mem::take(&mut *records.borrow_mut()))
}

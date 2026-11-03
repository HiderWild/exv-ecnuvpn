// Windows 实验owner：只借用underlay交来的实际DataNode，duplicate不另开连接。
// finish/Drop均停止并join采样线程；线程先结束采样、释放duplicate，再交回结果。
use serde_json::{Value, json};
use std::{
    os::windows::io::AsSocket,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(1);
const MAX_PERIODIC: usize = 120;
type Observer = Arc<dyn Fn(&tokio::net::TcpStream) + Send + Sync>;
type Probe = Box<dyn FnMut() -> Value + Send>;

#[derive(Default)]
struct State {
    finished: bool,
    attach_count: u32,
    attach_error: Option<Value>,
    worker: Option<Sampler>,
}

#[derive(Default)]
pub struct NodeTcpObservation {
    state: Arc<Mutex<State>>,
    completed: Option<(Value, bool)>,
}

impl NodeTcpObservation {
    pub fn observer(&self) -> Observer {
        let state = self.state.clone();
        Arc::new(move |stream| {
            let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
            if state.finished {
                return;
            }
            state.attach_count = state.attach_count.saturating_add(1);
            if state.attach_count != 1 {
                state.attach_error =
                    Some(json!({"stage":"attach","kind":"multiple_data_node_connections"}));
                return;
            }
            match stream.as_socket().try_clone_to_owned() {
                Ok(socket) => {
                    let probe = Box::new(move || crate::windows_tcp_info::sample(&socket));
                    match Sampler::spawn(probe, INTERVAL, MAX_PERIODIC) {
                        Ok(worker) => state.worker = Some(worker),
                        Err(error) => state.attach_error = Some(io_error("thread_spawn", &error)),
                    }
                }
                Err(error) => state.attach_error = Some(io_error("duplicate", &error)),
            }
        })
    }

    /// 顺序：stop/notify → join（线程内结束采样、drop duplicate）→ 允许签cleanup。
    pub fn finish(&mut self) -> (Value, bool) {
        if let Some(completed) = &self.completed {
            return completed.clone();
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.finished = true;
        let (mut value, cleanup) = state.worker.take().map_or_else(
            || (json!({"samples":null,"duplicate_released":true}), true),
            |mut worker| worker.finish(),
        );
        value["role"] = json!("DataNode");
        value["attach_count"] = json!(state.attach_count);
        value["attach_error"] = json!(state.attach_error);
        value["interval_ms"] = json!(INTERVAL.as_millis());
        value["max_periodic"] = json!(MAX_PERIODIC);
        if state.attach_count > 1 {
            // 本轮只允许一个被观测DataNode；不把首连接指标冒充后续连接。
            value["samples"] = Value::Null;
        }
        self.completed = Some((value.clone(), cleanup));
        (value, cleanup)
    }
}

impl Drop for NodeTcpObservation {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn io_error(stage: &str, error: &std::io::Error) -> Value {
    json!({"stage":stage,"os_error":error.raw_os_error(),"kind":format!("{:?}",error.kind())})
}

struct Sampler {
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Option<JoinHandle<Value>>,
}

impl Sampler {
    fn spawn(mut probe: Probe, interval: Duration, max_periodic: usize) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let stopping = stop.clone();
        let worker = std::thread::Builder::new().name("atrust_tcp_info".into()).spawn(move || {
            let started = Instant::now();
            let mut samples = Vec::with_capacity(max_periodic.saturating_add(2));
            let mut sample = |stage| {
                let value = probe();
                samples.push(json!({"stage":stage,"elapsed_us":started.elapsed().as_micros(),"tcp_info":value}));
            };
            sample("initial");
            for _ in 0..max_periodic {
                let (flag, wake) = &*stopping;
                let guard = flag.lock().unwrap_or_else(|error| error.into_inner());
                let (guard, timeout) = wake.wait_timeout_while(guard, interval, |stop| !*stop)
                    .unwrap_or_else(|error| error.into_inner());
                if *guard { break; }
                drop(guard);
                if timeout.timed_out() { sample("periodic"); }
            }
            // 达到有限采样上限后继续等待owner收尾，不提前生成结束样本。
            let (flag, wake) = &*stopping;
            let guard = flag.lock().unwrap_or_else(|error| error.into_inner());
            let guard = wake.wait_while(guard, |stop| !*stop).unwrap_or_else(|error| error.into_inner());
            drop(guard);
            sample("final");
            drop(sample);
            drop(probe); // 包含OwnedSocket；发生在join返回与cleanup签署之前。
            json!({"samples":samples,"duplicate_released":true})
        })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    fn finish(&mut self) -> (Value, bool) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(|error| error.into_inner()) = true;
        wake.notify_all();
        match self.worker.take().map(JoinHandle::join) {
            Some(Ok(value)) => (value, true),
            Some(Err(_)) => (
                json!({"samples":null,"collector_error":"sampler_panicked","duplicate_released":true}),
                true,
            ),
            None => (json!({"samples":null,"duplicate_released":true}), true),
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

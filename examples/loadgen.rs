//! Minimal HTTP load generator used by `scripts/benchmark.sh`.
//!
//! Kept in-tree so that both the Rust and the Python server are measured with
//! exactly the same client, on the same machine.
//!
//! ```bash
//! cargo run --release --example loadgen -- \
//!   --url http://127.0.0.1:18001/health --concurrency 32 --duration 15
//! ```
//!
//! The client is HTTP-only (no TLS) and keeps connections alive.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;

struct Args {
    url: String,
    method: String,
    body: Option<String>,
    headers: Vec<(String, String)>,
    concurrency: usize,
    duration: Duration,
    warmup: Duration,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut url = None;
        let mut method = "GET".to_string();
        let mut body = None;
        let mut headers = Vec::new();
        let mut concurrency = 32usize;
        let mut duration = Duration::from_secs(15);
        let mut warmup = Duration::from_secs(3);

        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--url" => url = Some(args.next().ok_or("--url needs a value")?),
                "--method" => method = args.next().ok_or("--method needs a value")?,
                "--body" => body = Some(args.next().ok_or("--body needs a value")?),
                "--header" => {
                    let raw = args.next().ok_or("--header needs a value")?;
                    let (name, value) = raw.split_once(':').ok_or("--header expects Name:Value")?;
                    headers.push((name.trim().to_string(), value.trim().to_string()));
                }
                "--concurrency" => {
                    concurrency = args
                        .next()
                        .ok_or("--concurrency needs a value")?
                        .parse()
                        .map_err(|_| "--concurrency must be a number")?;
                }
                "--duration" => {
                    duration = Duration::from_secs(
                        args.next()
                            .ok_or("--duration needs a value")?
                            .parse()
                            .map_err(|_| "--duration must be seconds")?,
                    );
                }
                "--warmup" => {
                    warmup = Duration::from_secs(
                        args.next()
                            .ok_or("--warmup needs a value")?
                            .parse()
                            .map_err(|_| "--warmup must be seconds")?,
                    );
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }

        Ok(Self {
            url: url.ok_or("--url is required")?,
            method,
            body,
            headers,
            concurrency: concurrency.max(1),
            duration,
            warmup,
        })
    }
}

struct Sample {
    latency: Duration,
    status: u16,
}

async fn run(client: reqwest::Client, args: std::sync::Arc<Args>, stop_at: Instant) -> Vec<Sample> {
    let method = reqwest::Method::from_bytes(args.method.as_bytes()).expect("valid method");
    let mut samples = Vec::new();
    while Instant::now() < stop_at {
        let mut request = client.request(method.clone(), &args.url);
        for (name, value) in &args.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(body) = &args.body {
            request = request.body(body.clone());
        }
        let started = Instant::now();
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                // Drain the body so the connection can be reused.
                let _ = response.bytes().await;
                samples.push(Sample {
                    latency: started.elapsed(),
                    status,
                });
            }
            Err(_) => samples.push(Sample {
                latency: started.elapsed(),
                status: 0,
            }),
        }
    }
    samples
}

fn percentile(sorted: &[u128], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[index] as f64 / 1000.0
}

#[tokio::main]
async fn main() {
    let args = std::sync::Arc::new(Args::parse().unwrap_or_else(|err| {
        eprintln!("error: {err}");
        std::process::exit(2);
    }));

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(args.concurrency)
        .tcp_nodelay(true)
        .build()
        .expect("build client");

    // Warm up: JIT, connection pools, caches.
    let warmup_end = Instant::now() + args.warmup;
    let mut tasks = Vec::new();
    for _ in 0..args.concurrency {
        tasks.push(tokio::spawn(run(
            client.clone(),
            std::sync::Arc::clone(&args),
            warmup_end,
        )));
    }
    for task in tasks {
        let _ = task.await;
    }

    let start = Instant::now();
    let stop_at = start + args.duration;
    let mut tasks = Vec::new();
    for _ in 0..args.concurrency {
        tasks.push(tokio::spawn(run(
            client.clone(),
            std::sync::Arc::clone(&args),
            stop_at,
        )));
    }
    let mut samples = Vec::new();
    for task in tasks {
        if let Ok(collected) = task.await {
            samples.extend(collected);
        }
    }
    let elapsed = start.elapsed().as_secs_f64();

    let total = samples.len();
    let mut statuses: BTreeMap<u16, usize> = BTreeMap::new();
    let mut latencies: Vec<u128> = Vec::with_capacity(total);
    for sample in &samples {
        *statuses.entry(sample.status).or_default() += 1;
        latencies.push(sample.latency.as_micros());
    }
    latencies.sort_unstable();

    // Transport failures (status 0) and server errors count as errors; the
    // 4xx responses used by the validation scenarios are expected outcomes.
    let errors = samples
        .iter()
        .filter(|sample| sample.status == 0 || sample.status >= 500)
        .count();
    let rps = total as f64 / elapsed;

    println!(
        "{}",
        json!({
            "url": args.url,
            "method": args.method,
            "concurrency": args.concurrency,
            "duration_s": elapsed.round(),
            "requests": total,
            "rps": (rps * 10.0).round() / 10.0,
            "errors": errors,
            "error_rate": if total == 0 { 0.0 } else { (errors as f64 * 10000.0 / total as f64).round() / 100.0 },
            "latency_ms": {
                "avg": if total == 0 { 0.0 } else { (latencies.iter().sum::<u128>() as f64 / total as f64) / 1000.0 },
                "p50": percentile(&latencies, 0.50),
                "p90": percentile(&latencies, 0.90),
                "p99": percentile(&latencies, 0.99),
                "max": percentile(&latencies, 1.0),
            },
            "statuses": statuses,
        })
    );
}

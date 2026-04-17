//! End-to-end scan benchmark over a synthetic 1,400-skill library.

use std::fs;
use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use skilldigest::audit::{self, AuditOptions};
use skilldigest::model::BudgetConfig;
use skilldigest::scan::ScanPolicy;
use skilldigest::tokenize;

fn build_synthetic(root: &Path, n: usize) {
    for i in 0..n {
        let rel = format!("bucket{}/skill-{i:04}/SKILL.md", i % 20);
        let path = root.join(&rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body = format!(
            "---\nname: skill-{i:04}\ntags:\n  - cat{}\n---\n\
# Skill {i}\n\n\
This is a synthetic skill used for the skilldigest benchmark. Use `Bash(ls)` \
and `Edit(foo.md)` for local edits. NEVER use `Write(/etc/*)`.\n",
            i % 10
        );
        fs::write(path, body).unwrap();
    }
}

fn bench_scan_1400(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    build_synthetic(dir.path(), 1400);
    c.bench_function("scan_1400_skills", |b| {
        b.iter(|| {
            let options = AuditOptions {
                root: dir.path().to_path_buf(),
                tokenizer: tokenize::by_name("cl100k").unwrap(),
                budget: BudgetConfig {
                    per_skill: 2000,
                    total: None,
                },
                policy: ScanPolicy::default(),
                overrides: Default::default(),
            };
            let report = audit::run(black_box(options)).unwrap();
            black_box(report.total_skills);
        })
    });
}

criterion_group!(benches, bench_scan_1400);
criterion_main!(benches);

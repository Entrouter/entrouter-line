use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use entrouter_line::relay::crypto::{self, TunnelCrypto};
use entrouter_line::relay::fec::{FecConfig, FecEncoder};
use entrouter_line::relay::wire;

fn bench_encrypt_1400(c: &mut Criterion) {
    let key = crypto::generate_key();
    let crypto = TunnelCrypto::new(&key);
    let payload = vec![0xABu8; 1400]; // MTU-sized payload

    let mut group = c.benchmark_group("encrypt");
    group.throughput(Throughput::Bytes(1400));
    group.bench_function("chacha20poly1305_1400B", |b| {
        b.iter(|| {
            let _ = black_box(crypto.encrypt(42, &payload));
        });
    });
    group.finish();
}

fn bench_decrypt_1400(c: &mut Criterion) {
    let key = crypto::generate_key();
    let crypto = TunnelCrypto::new(&key);
    let payload = vec![0xABu8; 1400];
    let ciphertext = crypto.encrypt(42, &payload);

    let mut group = c.benchmark_group("decrypt");
    group.throughput(Throughput::Bytes(1400));
    group.bench_function("chacha20poly1305_1400B", |b| {
        b.iter(|| {
            let _ = black_box(crypto.decrypt(42, &ciphertext));
        });
    });
    group.finish();
}

fn bench_fec_encode(c: &mut Criterion) {
    let config = FecConfig {
        data_shards: 6,
        parity_shards: 4,
    };
    let encoder = FecEncoder::new(config);
    let shard_size = 700; // Typical shard size for 4KB block

    let mut group = c.benchmark_group("fec_encode");
    group.throughput(Throughput::Bytes((shard_size * 6) as u64));
    group.bench_function("reed_solomon_6+4_700B", |b| {
        b.iter(|| {
            let mut shards: Vec<Vec<u8>> = (0..6).map(|i| vec![i as u8; shard_size]).collect();
            encoder.encode(&mut shards);
            black_box(&shards);
        });
    });
    group.finish();
}

fn bench_fec_reconstruct(c: &mut Criterion) {
    let config = FecConfig {
        data_shards: 6,
        parity_shards: 4,
    };
    let encoder = FecEncoder::new(config);
    let shard_size = 700;

    // Pre-encode
    let mut shards: Vec<Vec<u8>> = (0..6).map(|i| vec![i as u8; shard_size]).collect();
    encoder.encode(&mut shards);

    let mut group = c.benchmark_group("fec_reconstruct");
    group.throughput(Throughput::Bytes((shard_size * 6) as u64));
    group.bench_function("recover_2_lost_of_10", |b| {
        b.iter(|| {
            let mut opt: Vec<Option<Vec<u8>>> = shards.iter().cloned().map(Some).collect();
            opt[0] = None;
            opt[3] = None;
            let _ = black_box(encoder.reconstruct(&mut opt));
        });
    });
    group.finish();
}

fn bench_wire_framing(c: &mut Criterion) {
    let mut buf = [0u8; wire::MAX_PACKET];

    let mut group = c.benchmark_group("wire");
    group.bench_function("encode_header", |b| {
        b.iter(|| {
            wire::encode_header(&mut buf, wire::PACKET_DATA, 1234, 1400);
            black_box(&buf);
        });
    });
    group.bench_function("decode_header", |b| {
        wire::encode_header(&mut buf, wire::PACKET_DATA, 1234, 1400);
        b.iter(|| {
            let _ = black_box(wire::decode_header(&buf));
        });
    });
    group.finish();
}

fn bench_full_pipeline(c: &mut Criterion) {
    let key = crypto::generate_key();
    let crypto = TunnelCrypto::new(&key);
    let config = FecConfig {
        data_shards: 6,
        parity_shards: 4,
    };
    let encoder = FecEncoder::new(config);
    let data = vec![0xCDu8; 4200]; // 4.2KB block

    let mut group = c.benchmark_group("full_pipeline");
    group.throughput(Throughput::Bytes(4200));
    group.bench_function("fec_encode+encrypt_4200B", |b| {
        b.iter(|| {
            let shard_size = (data.len() + config.data_shards - 1) / config.data_shards;
            let mut shards: Vec<Vec<u8>> = data
                .chunks(shard_size)
                .map(|c| {
                    let mut s = c.to_vec();
                    s.resize(shard_size, 0);
                    s
                })
                .collect();
            while shards.len() < config.data_shards {
                shards.push(vec![0u8; shard_size]);
            }
            encoder.encode(&mut shards);

            // Encrypt each shard
            for (i, shard) in shards.iter().enumerate() {
                let ct = crypto.encrypt(i as u64, shard);
                let mut frame = vec![0u8; wire::HEADER_SIZE + ct.len()];
                wire::encode_header(&mut frame, wire::PACKET_DATA, i as u64, ct.len() as u16);
                frame[wire::HEADER_SIZE..].copy_from_slice(&ct);
                black_box(&frame);
            }
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_encrypt_1400,
    bench_decrypt_1400,
    bench_fec_encode,
    bench_fec_reconstruct,
    bench_wire_framing,
    bench_full_pipeline,
);
criterion_main!(benches);

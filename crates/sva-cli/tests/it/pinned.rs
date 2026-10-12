// Concern: pins the bytes, store keys and readings fixed compositions render to | Non-concern: `atoms`, why any value is right | IO: (fixtures/*) -> hashes

use std::path::Path;
use std::process::Command;

use crate::helpers::{fixture, scratch};

/// Each case: a fixture, a target over a closed interval, and the one instance `bindings` reads.
const CASES: [(&str, &str, &str); 17] = [
    ("arranged", "@master([0, 1s])", "@master"),
    ("bar-grid", "@master([0, 1s])", "@master"),
    ("basic", "@master([0, 1s])", "@master"),
    ("basic", "@loop-1b([0, 0.5s])", "@loop-1b"),
    ("cost", "@bowl([0, 0.1s])", "@bowl"),
    ("no-tempo", "@master([0, 1s])", "@master"),
    ("reverb", "@master([0, 0.5s])", "@fx/reverb"),
    ("stereo", "@master([0, 0.06s])", "@pan"),
    ("stereo", "@pingpong([0, 0.6s])", "@pingpong"),
    ("two-entries", "@master([0, 1s])", "@master"),
    ("two-entries", "@bench-2s([0, 1s])", "@bench-2s"),
    (
        "closure-1a4989c6",
        "@sva1a4989c6fb22b2969fa478e6ee47b16aee6a20df52c484d241618c6b112876bb([0, 1s])",
        "@sva4065c0245b85ba8686d4da12124b41e3d0ea09ebb3000a229f931ee6194a9558(f0=D2)",
    ),
    (
        "closure-1e362f88",
        "@sva1e362f88072bf4bb3c0edb42fda6269ffc00ee12c4f3f4e07bbde0269c103ded([0, 1s])",
        "@sva5f25539e1ff16edd2ae8228c54964a2afe65f15881c456fdcaf0a4d36ec52082(f0=A1, release=0.8, vel=1)",
    ),
    (
        "closure-1f15ca3d",
        "@sva1f15ca3db2096f385f391721c5c0fff2210e4c6b3ed996ef063529631ac3bda6([0, 1s])",
        "@svad163ed7735e112fc74e76ed73b689901236c80717584dcc13d14fb72cd3f0e4e(seed=1)",
    ),
    (
        "closure-db781e56",
        "@svadb781e56ae75d7bb4802f257ff1b7c509b9cce9dcd53f4cad13450110d3a4a8f([0, 0.5s])",
        "@sva8062dfa9b6df789c110d873e7fe5164b6ab95b0c7517ca5dcb7afce047d0f9bc(f0=261.6255653006, release=3.0833333333, vel=0.7087)",
    ),
    (
        "closure-db781e56",
        "@sva046a5fc7c03eb08d8ac01e517b4ce72a7765d820d9966e676c197a21b3b39045([0, 0.1s], tc=0.08)",
        "@sva046a5fc7c03eb08d8ac01e517b4ce72a7765d820d9966e676c197a21b3b39045",
    ),
    (
        "closure-f5a155b1",
        "@svaf5a155b1d5f640ea99aa441093bdb8c793a166a697f1f1eb8c032160feb4cd81([0, 0.5s])",
        "@sva3828c5b584260ff15968d7fd155d5d8856184bd78099c66fca8b2b39f671b8b5(f0=A2, release=2, vel=0.35)",
    ),
];

/// Every reading but `atoms`, and `samples`, which the `.wav` pins.
const READINGS: [&str; 15] = [
    "lines",
    "spectrum",
    "envelope",
    "derivative",
    "ledger",
    "pitch",
    "formants",
    "stereo",
    "bands",
    "crest",
    "loudness",
    "onsets",
    "alias",
    "bindings",
    "arguments",
];

/// One line per case and item, `<case> <item> <hash, or the codes it refused>`.
const PINNED: &str = r#"
0 stdout 247a8ad492f8b3d7
0 wav 665924a7a274d695
0 store:3620e0612b5d84925fa1bd15f1b7716e 1fe8fe57e72ce51b
0 lines refused "read.lines_need_unwindowed_lines"
0 spectrum refused "read.lines_need_unwindowed_lines"
0 envelope refused "cast.left_algebra"
0 derivative 3c4b7a53de255642
0 ledger 301ba8530bb8a641
0 pitch refused "read.lines_need_unwindowed_lines"
0 formants 69ac51091d002826
0 stereo refused "type.width_mismatch"
0 bands 5ddbbb246c27967f
0 crest 9da293a2a033e041
0 loudness 8546363c7f1ff81e
0 onsets 5d9e390a40d4416c
0 alias 12566b98eb3a9c25
0 bindings b2361ce7b3484da7
0 arguments 2629850681a29371
1 stdout 247a8ad492f8b3d7
1 wav 26291e77cc78e829
1 store:90af903a105917870539e1943a4b72c7 86c1e3c035d49daa
1 lines refused "read.lines_need_unwindowed_lines"
1 spectrum refused "read.lines_need_unwindowed_lines"
1 envelope refused "cast.left_algebra"
1 derivative acfd9df607f9e976
1 ledger 40b6413cf9c33fc0
1 pitch refused "read.lines_need_unwindowed_lines"
1 formants 4558f476fdf28802
1 stereo refused "type.width_mismatch"
1 bands 652286a71d7ba909
1 crest 6589fb8644e10ca3
1 loudness 85260bff5b79b87e
1 onsets 2a805c518cf4c8ca
1 alias 4b29f3d8f7553303
1 bindings b2361ce7b3484da7
1 arguments 2629850681a29371
2 stdout 247a8ad492f8b3d7
2 wav 968e2e9858e90ed9
2 store:29c7e17ebaef85e9ff784832c3108192 a2a01940bd0dd189
2 lines refused "read.no_spectral_sum"
2 spectrum refused "read.no_spectral_sum"
2 envelope refused "read.no_spectral_sum"
2 derivative refused "read.no_spectral_sum"
2 ledger d5403c5358edc0e2
2 pitch refused "read.no_spectral_sum"
2 formants c3364258e02d81e6
2 stereo refused "type.width_mismatch"
2 bands f7a5e849debf50b2
2 crest fbd77b10d5dde3ac
2 loudness 606a38acc7131c55
2 onsets bbda566d6a45cbdd
2 alias fdf260e22e2a1011
2 bindings b2361ce7b3484da7
2 arguments 626f051e3a2fc700
3 stdout ab995f31111d716c
3 wav 64318e943f03cc25
3 store:273d32d1fbc24d73b322a79899449566 7413c92cbb75c060
3 lines 1b91625484873716
3 spectrum 72763f1617db3c1e
3 envelope 340b0cda41fcac70
3 derivative 7543c4c4781ded5c
3 ledger 5c22057f215775d1
3 pitch 31eb906bd2504ddd
3 formants 82cfccabbf2a95e5
3 stereo refused "type.width_mismatch"
3 bands 3d2781a6afdfb4c8
3 crest e0f4c5aaa9c7c270
3 loudness 6e1791349e7075fa
3 onsets 7c4821e78364f0a2
3 alias 02979bbbe4119071
3 bindings de379a53261f899f
3 arguments 963d7e681d3a7159
4 stdout 53ef6500c0c74fac
4 wav d8d646db74dc9cb2
4 store:7d17584ff33a1d5714e10c35ba698e01 6d6a4a3c724606e5
4 lines 856b8c259fc6ee26
4 spectrum 5875cbec9753bd9a
4 envelope 2c3741c76dd8c27d
4 derivative 1335b1af4f591c60
4 ledger c053925e93af266d
4 pitch 12415b8ef26ea22e
4 formants 5b0b91afdc94a81c
4 stereo refused "type.width_mismatch"
4 bands eaa2cac9886e0d17
4 crest 4b17dbafe5a25887
4 loudness f43469c2b46c18fa
4 onsets c36ba45956b46a29
4 alias 6d697a7dac4b06e4
4 bindings a066a34dcc83fcfb
4 arguments 9eaefeccb255ec5d
5 stdout 247a8ad492f8b3d7
5 wav 177aed0c28183ef5
5 store:52690610e792cbd9ec87410effce6ac0 38434ca82780e042
5 lines 4745742b132f215a
5 spectrum cfc00a7586fe2dc2
5 envelope 310ce19eaa76ea44
5 derivative 71802c56289d0318
5 ledger e4349ebf63d76218
5 pitch 6c4088af788a0ba4
5 formants 7a25e231d6e83128
5 stereo refused "type.width_mismatch"
5 bands 41f346248f013bb4
5 crest 7e413e4086bc977c
5 loudness 8e4f1c0baecfe087
5 onsets bed186cf1d6b199f
5 alias f133ed27246b0558
5 bindings b2361ce7b3484da7
5 arguments 2629850681a29371
6 stdout b7be809aeee4a986
6 wav 72bce77b8424e0c6
6 store:0aa625a325ad009e42736fb0b55da15e 3e75968a1c2ea96b
6 store:11c136c343d5887a356999a4fc86e8f7 6c39d5eac4b4722a
6 store:14f6414df6fcdd24e3ba1efa6b601e02 3a393e43b256bf25
6 store:2a812a178319a3d66188660d033eee56 ce921026831f6b1b
6 store:2b1dfb572460ba3c3e8a08891ad3feda 966de9046403b2cd
6 store:2cc1e056c0cd5c6013c986cb46ea8680 08f23650cc321c8c
6 store:57a6b91a9d6ee92c9f220a471c761060 da9cea10e63b03d5
6 store:6e22a9b54e73a673f2f7252c439f5d3c 3bf6ea6ddf766000
6 store:6f9bcd032cb9677f874eba226b581950 199ab152acdff898
6 store:d18c8f806552ff35a1cabe95f566fe2e e0e016d44964d24b
6 store:d4b554f1adbd1e2708edf6cef39dd919 a323d7d803bef056
6 store:dfbe3e234fa8c68c2717f78df327e90f a258d9a132303ece
6 store:e41ecc22b1d8d907ef9ba75bd804e1e7 aa4f8111a5fb54db
6 store:e51a27e86bcbd36a2c884364cb358a1e 67aa5fae4adaef7b
6 lines refused "type.samples_in_closed_form"
6 spectrum 0e7eda97fd299e30
6 envelope 510b9276d5e7c870
6 derivative 2ada75b05fe73a68
6 ledger e296c7b26fd92e2f
6 pitch 916761bc560ab4ec
6 formants 4a5877c3e5e367d9
6 stereo refused "type.width_mismatch"
6 bands 5f534aa9aa3bbcd7
6 crest 0ee986c4ebc6919e
6 loudness cb5e566b2cbf9109
6 onsets 4b161b614474913d
6 alias refused "engine.alias_needs_a_closed_form"
6 bindings 61d25ef94a8f6635
6 arguments f96cc4a110318b3d
7 stdout edd1ad4d03ca1433
7 wav 3372a403bff9f10a
7 store:48cc333eda6f48499d525bb87ae7a03c 8267ef7eb946b055
7 lines refused "read.lines_need_unwindowed_lines"
7 spectrum refused "read.lines_need_unwindowed_lines"
7 envelope refused "cast.left_algebra"
7 derivative 4c8306ae0e47500b
7 ledger 3eced3ce79e67005
7 pitch refused "read.lines_need_unwindowed_lines"
7 formants 92c165924f4850c5
7 stereo ed61419626773d93
7 bands b1152050da45e57d
7 crest 046c981ae06261fc
7 loudness 7d6440ef7def4aca
7 onsets 110fd55908dbed15
7 alias 2e6ca0047cafaf68
7 bindings e78c34f33117e96f
7 arguments bcbf8809de18a142
8 stdout ab6aa7bed370fd04
8 wav 083e3fdfa6fd2b2d
8 store:0435c2987fec5f6fa5f7bc60ed42587e 004d1d0eb25c2b76
8 lines refused "type.samples_in_closed_form"
8 spectrum 81d5aa4c53750acc
8 envelope 0c5253e952c2fa8d
8 derivative d3faa20b9834549e
8 ledger d7d7417544077963
8 pitch 3736b94f2701483a
8 formants 91826cb66b084d64
8 stereo 6494bdf51a307118
8 bands a3ee3343e5d5994c
8 crest fb94330cd10aafeb
8 loudness 470df9d7cf11cbe3
8 onsets e7be24fbc28749b4
8 alias refused "engine.alias_needs_a_closed_form"
8 bindings 79396b2f23896766
8 arguments a5fe2d9cfe6ecec2
9 stdout 247a8ad492f8b3d7
9 wav 3a9ca1a53a5fa42a
9 store:3180ee7f5ba55875b73514cbc66c3107 cccdd9eafa5f76a7
9 lines refused "read.lines_need_unwindowed_lines"
9 spectrum refused "read.lines_need_unwindowed_lines"
9 envelope refused "cast.left_algebra"
9 derivative 832b940b443c04d9
9 ledger 01194d6093d32b20
9 pitch refused "read.lines_need_unwindowed_lines"
9 formants 4a1893055be9d546
9 stereo refused "type.width_mismatch"
9 bands 21d75879d5a36084
9 crest 17fd2a6d80a8ce3a
9 loudness b0dbfaeaf9dddfe1
9 onsets 82baf2656854e410
9 alias 95e3d9a0d16a8cbd
9 bindings b2361ce7b3484da7
9 arguments 2629850681a29371
10 stdout 099b2f66421d30cb
10 wav a9866247325c001f
10 store:6655845ef1b852c2dfba6667b7a17cb7 e819f0f8c841a947
10 lines refused "read.lines_need_unwindowed_lines"
10 spectrum refused "read.lines_need_unwindowed_lines"
10 envelope refused "cast.left_algebra"
10 derivative 3b93523aba257d86
10 ledger d85440a165c53e75
10 pitch refused "read.lines_need_unwindowed_lines"
10 formants a89662a64fb8a4dd
10 stereo refused "type.width_mismatch"
10 bands 198cf5dfb1458000
10 crest bafbab9959c18d6e
10 loudness 5357a6cb2a187221
10 onsets 0ee70062d8db3514
10 alias 94339908ea31be4d
10 bindings 5d16a4e7b16c54d3
10 arguments 13a9d91a5de370e5
11 stdout d79a956da40777a2
11 wav c187b7408a55ebf7
11 store:05527852d844ba60e2f03bfc763cbeda 185c5d789d9c2e42
11 store:b6774780ba26bd24289ee5a5a363539e 761f7268801ab923
11 store:fef920a29d25f7e69ab776f1c1aa2df8 0c081ff29816c5bd
11 lines refused "type.samples_in_closed_form"
11 spectrum d0ea49e9c415d55c
11 envelope 7c67a5d377431f4e
11 derivative 0234ccb44aa74534
11 ledger 85f91208b93bf675
11 pitch 4b1e30edd8024b2e
11 formants ab893e9880a0ad66
11 stereo refused "type.width_mismatch"
11 bands 8f984c522da9befa
11 crest d2139cfcaf0fd8ba
11 loudness 66d733b1b2e46ac9
11 onsets 9c5589dfaa333a9f
11 alias refused "engine.alias_needs_a_closed_form"
11 bindings 6187812054881a9c
11 arguments 00523829ab775155
12 stdout 2125d7cc6a62b570
12 wav 3fc9f322aaf6fc5f
12 store:05fd9b34f0c3d96a4508e7fe93cef407 2c75910dfef01763
12 store:eefd7456adda71f95410a477bc38064f dd611b0c27105544
12 lines refused "type.samples_in_closed_form"
12 spectrum f7ce98968fff277d
12 envelope 1b9d192b3e91dcb6
12 derivative 52556069c8911ad2
12 ledger 9521693f05bf90e3
12 pitch ad91031a72ab646d
12 formants f63e3f59fb8ff19a
12 stereo refused "type.width_mismatch"
12 bands 8d953bbc6e7a6774
12 crest 93883cb32f667560
12 loudness 2fb8be9dcf204d31
12 onsets b8daac46e3eb10c2
12 alias refused "engine.alias_needs_a_closed_form"
12 bindings 2246221dde6bf19e
12 arguments 31ac7203e7c98531
13 stdout 35b22cc894edba26
13 wav 256b6dfecab93743
13 store:7766d37a0cdcc294553055f59415b4ee 0eda0b656511230e
13 store:b636f740c7f5c69c7d133075cf6253c3 a2cd706d6e684ffd
13 lines refused "type.samples_in_closed_form"
13 spectrum 0c84a510941b3623
13 envelope c7477734fba002a5
13 derivative e43d66b13cd27645
13 ledger 1a738c8d556a802e
13 pitch bcdc0c08ebe1e42a
13 formants e82d2ddad859d9c3
13 stereo refused "type.width_mismatch"
13 bands ff24830f55ca7d97
13 crest 9bad027881a738b0
13 loudness 8cdcf4285fbb7827
13 onsets ca58478c30e14e96
13 alias refused "engine.alias_needs_a_closed_form"
13 bindings 4d733f3c0aabb4f4
13 arguments c4f0bea827f62da3
14 stdout cfe7231299dd5013
14 wav 0c1d8e0ad4b6a019
14 store:254945568a381e7c3e5471da1d74728f 5494cf24093fc910
14 store:9799ab7ba6f836a107fd71777f42e6f0 4b17c1b594050d6d
14 store:979e8f53e59fb8d66035a41b9132d6a7 904d61701883693f
14 store:f9f5071eb92a34c3a0ca9fe84c15c685 fa2022a411879c9b
14 lines refused "type.samples_in_closed_form"
14 spectrum b23635798e5862ea
14 envelope 88f602a703affc7c
14 derivative 2f09043a08b9ee72
14 ledger 3ce3405aef9719b3
14 pitch e4528f67c76d7459
14 formants 07b076a9b2773249
14 stereo refused "type.width_mismatch"
14 bands e59cf9c09fde48fd
14 crest 946716d90df65745
14 loudness 88bf6f18c551beaf
14 onsets 0747a61d69a01713
14 alias refused "engine.alias_needs_a_closed_form"
14 bindings 93b2bd559b5e577e
14 arguments 7309f5cc8a34fe8a
15 stdout c1ee5cc59d6fb404
15 wav 82773cac15ff1cca
15 store:b5146c74a37c137f36a90b33af43d7d6 439fae25c8a42cf5
15 lines refused "read.lines_need_unwindowed_lines"
15 spectrum refused "read.lines_need_unwindowed_lines"
15 envelope refused "cast.left_algebra"
15 derivative 85fd74bc6d856544
15 ledger f3afb30b0c4c2fb5
15 pitch refused "read.lines_need_unwindowed_lines"
15 formants 53fe55520cbb1aba
15 stereo refused "type.width_mismatch"
15 bands 783c04ff4dc38ba4
15 crest 25f0c999bb9f0512
15 loudness 46d79f3bbbb5df11
15 onsets 30148b49712da1ff
15 alias 88aa3394e42660fb
15 bindings f758b54150146934
15 arguments 6fe5313a42a03884
16 stdout 81e5860d85938ad0
16 wav 9007d5d55c305e67
16 store:26ad75243b2f79c4c406c6f5bf4ec486 147a6b22cb539a1c
16 store:3c3e02ce6997bbbc519549daeed604f4 1fdf1c279cacfa5d
16 store:721873409668da2d147b4a20e9a0d6e3 0420ffa38a043277
16 store:7d2a34d66251dbf649d696726b2f90f8 70abf1f5da6338b6
16 store:7febd1dbb83c13997bf57fbce8e72f50 65c926d213157225
16 lines refused "type.samples_in_closed_form"
16 spectrum 547386550b46a3a7
16 envelope eedb58d4e0ba84e8
16 derivative 70b92325601c8608
16 ledger e9282e1791e85be2
16 pitch c34b5010d79ff1a8
16 formants 6f05c52bc183f345
16 stereo refused "type.width_mismatch"
16 bands a3d90ca6090450f6
16 crest 45a7abc702e4a819
16 loudness 17d3cd1a81f00579
16 onsets 0992a2bb7e332ca1
16 alias refused "engine.alias_needs_a_closed_form"
16 bindings 76ea2f9c67cc302d
16 arguments ef9f32c3d73615ee
"#;

/// FNV-1a: stable across toolchains, which `DefaultHasher` does not promise.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The envelope before its `meta`, which differs per run.
fn before_meta(printed: &str) -> &str {
    &printed[..printed.find("\"meta\"").expect("an envelope")]
}

/// One `sva-cli render` of `target` in `dir` over the store at `store`: whether it answered,
/// and its stdout before `meta`.
fn render(dir: &Path, target: &str, store: &Path, representation: &str) -> (bool, String) {
    let ran = Command::new(env!("CARGO_BIN_EXE_sva-cli"))
        .current_dir(dir)
        .args([
            "render",
            target,
            "--representation",
            representation,
            "--cache",
        ])
        .arg(store)
        .output()
        .expect("the binary runs");
    let printed = String::from_utf8_lossy(&ran.stdout);
    (ran.status.success(), before_meta(&printed).to_string())
}

/// The codes a refusal names, which a caller branches on, and never the wording around them.
fn codes(printed: &str) -> &str {
    let opening = "\"codes\": { \"items\": [";
    let from = printed.find(opening).expect("a refusal's codes") + opening.len();
    let listed = &printed[from..];
    &listed[..listed.find(']').expect("a closed array")]
}

/// Each value the store holds, under its key.
fn stored(store: &Path) -> Vec<(String, String)> {
    let mut keys: Vec<String> = std::fs::read_dir(store)
        .expect("a store")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            let bytes = std::fs::read(store.join(&key)).expect("a stored value");
            (format!("store:{key}"), format!("{:016x}", fnv(&bytes)))
        })
        .collect()
}

/// The CLI's render of one case: its stdout with the scratch path taken out, its samples as a
/// `.wav`, the store that render left, then each reading off that store, or what it refused.
fn pinned(case: usize, dir: &Path, target: &str, instance: &str) -> Vec<(String, String)> {
    let out = scratch(&format!("pinned-{case}"));
    let wav = out.join("out.wav");
    let store = out.join("store");
    let (answered, stdout) = render(dir, target, &store, &format!("samples={}", wav.display()));
    let samples = std::fs::read(&wav).unwrap_or_default();
    let mut held = vec![
        (
            "stdout".to_string(),
            format!(
                "{:016x}",
                fnv(stdout
                    .replace(&wav.display().to_string(), "<wav>")
                    .as_bytes())
            ),
        ),
        ("wav".to_string(), format!("{:016x}", fnv(&samples))),
    ];
    held.extend(stored(&store));
    for name in READINGS {
        let call = match name {
            "bindings" => format!("bindings(node={instance})"),
            other => other.to_string(),
        };
        let (read, printed) = render(dir, target, &store, &call);
        let value = match read {
            true => format!("{:016x}", fnv(printed.as_bytes())),
            false => format!("refused {}", codes(&printed)),
        };
        held.push((name.to_string(), value));
    }
    let _ = std::fs::remove_dir_all(&out);
    assert!(answered, "{target}: {stdout}");
    held
}

/// Bytes, stored values and every reading but `atoms`, per case. A failure prints what moved
/// and the whole table, which replaces `PINNED` after a change meant to move it. Pinned on
/// x86_64 Linux and the pinned toolchain alone: another libm may round a sample differently.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn renders_stores_and_readings_answer_what_they_answered() {
    let mut table = String::new();
    for (case, (name, target, instance)) in CASES.iter().enumerate() {
        for (item, value) in pinned(case, &fixture(name), target, instance) {
            table.push_str(&format!("{case} {item} {value}\n"));
        }
    }
    let moved: Vec<&str> = table
        .lines()
        .filter(|line| !PINNED.trim().lines().any(|p| p == *line))
        .collect();
    assert!(
        moved.is_empty() && table.lines().count() == PINNED.trim().lines().count(),
        "moved:\n{}\nwhole table:\n{table}",
        moved.join("\n")
    );
}

use assert_cmd::Command;
use predicates::str::contains;
use rstest::rstest;
use serial_test::file_serial;

#[rstest]
#[case("1:1", "1BgGZ9tcN4rm9KBzDn7KprQz87SZ26SAMH")]
#[case("2:3", "1CUNEBjYrCn2y1SdiUMohaKUi4wpP326Lb")]
#[case("4:7", "19ZewH8Kk1PDbSNdJ97FP4EiCjTRaZMZQA")]
#[case("8:f", "1EhqbyUMvvs7BfL8goY6qcPbD6YKfPqb7e")]
#[case("10:1f", "1E6NuFjCi27W5zoXg8TRdcSRq84zJeBW3k")]
#[case("20:3f", "1PitScNLyp2HCygzadCh7FveTnfmpPbfp8")]
#[case("40:7f", "1McVt1vMtCC7yn5b9wgX1833yCcLXzueeC")]
#[case("80:ff", "1M92tSqNmQLYw33fuBvjmeadirh1ysMBxK")]
#[case("100:1ff", "1CQFwcjw1dwhtkVWBttNLDtqL7ivBonGPV")]
#[case("200:3ff", "1LeBZP5QCwwgXRtmVUvTVrraqPUokyLHqe")]
#[case("400:7ff", "1PgQVLmst3Z314JrQn5TNiys8Hc38TcXJu")]
#[case("800:fff", "1DBaumZxUkM4qMQRt2LVWyFJq5kDtSZQot")]
#[case("1000:1fff", "1Pie8JkxBT6MGPz9Nvi3fsPkr2D8q3GBc1")]
#[case("2000:3fff", "1ErZWg5cFCe4Vw5BzgfzB74VNLaXEiEkhk")]
#[case("4000:7fff", "1QCbW9HWnwQWiQqVo5exhAnmfqKRrCRsvW")]
#[case("8000:ffff", "1BDyrQ6WoF8VN3g9SAS1iKZcPzFfnDVieY")]
#[case("10000:1ffff", "1HduPEXZRdG26SUT5Yk83mLkPyjnZuJ7Bm")]
#[case("20000:3ffff", "1GnNTmTVLZiqQfLbAdp9DVdicEnB5GoERE")]
#[case("40000:7ffff", "1NWmZRpHH4XSPwsW6dsS3nrNWfL1yrJj4w")]
#[case("80000:fffff", "1HsMJxNiV7TLxmoF6uJNkydxPFDog4NQum")] // 20
#[ignore] // Heavy GPU/CPU test; run manually: cargo test -- --ignored --nocapture
#[file_serial(gpu)] // all tests with the same name run one-at-a-time across binaries
fn finds_known_address(#[case] range: &str, #[case] target: &str) {
    let mut cmd = Command::cargo_bin("gpu-bitcrack").unwrap();
    cmd.arg(range).arg(target).arg("--batch").arg("1000000"); // 1 million candidates per GPU dispatch

    cmd.assert()
        .success()
        .stdout(contains("FOUND!"))
        .stdout(contains(target));
}

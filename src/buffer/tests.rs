use crate::buffer::{LineEnding, line_ending_iter};
use memchr::memmem::Finder;

#[test]
fn test_single_line() {
    let s = b"hello";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"hello");
    assert_eq!(res[0].1, b"hello");
    assert_eq!(res[0].2, 0..5);
}

#[test]
fn test_simple_lines() {
    let s = b"foo\nbar\nbaz";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"foo");
    assert_eq!(res[0].1, b"foo\n");
    assert_eq!(res[0].2, 0..4);
    assert_eq!(res[1].0, b"bar");
    assert_eq!(res[1].1, b"bar\n");
    assert_eq!(res[1].2, 4..8);
    assert_eq!(res[2].0, b"baz");
    assert_eq!(res[2].1, b"baz");
    assert_eq!(res[2].2, 8..11);
}

#[test]
fn test_few_bytes() {
    let s = b"a";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"a");
    assert_eq!(res[0].1, b"a");

    let s = b"";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"");
    assert_eq!(res[0].1, b"");
}

#[test]
fn test_trailing_newline() {
    let s = b"a\nb\nc\n";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"a");
    assert_eq!(res[0].1, b"a\n");
    assert_eq!(res[1].0, b"b");
    assert_eq!(res[1].1, b"b\n");
    assert_eq!(res[2].0, b"c");
    assert_eq!(res[2].1, b"c\n");
}

#[test]
fn test_starting_newline() {
    let s = b"\rb\nc\n";
    let le = LineEnding::Byte(b'\r');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].0, b"");
    assert_eq!(res[0].1, b"\r");
    assert_eq!(res[1].0, b"b\nc\n");
    assert_eq!(res[1].1, b"b\nc\n");
}

#[test]
fn test_crlf() {
    let s = b"one\r\ntwo\r\nthree";
    let le = LineEnding::MultiByte(Finder::new(b"\r\n"));
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"one");
    assert_eq!(res[0].1, b"one\r\n");
    assert_eq!(res[1].0, b"two");
    assert_eq!(res[1].1, b"two\r\n");
    assert_eq!(res[2].0, b"three");
    assert_eq!(res[2].1, b"three");
}

#[test]
#[should_panic(expected = "empty finder")]
fn test_line_ending_empty() {
    let s = b"test";
    let le = LineEnding::MultiByte(Finder::new(&[]));
    let _ = line_ending_iter(s, &le);
}

#[test]
fn test_multi_byte_line_ending() {
    let s = b"abcXYZdefXYZghi";
    let finder = Finder::new(b"XYZ");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"abc");
    assert_eq!(res[0].1, b"abcXYZ");
    assert_eq!(res[1].0, b"def");
    assert_eq!(res[1].1, b"defXYZ");
    assert_eq!(res[2].0, b"ghi");
    assert_eq!(res[2].1, b"ghi");
}

#[test]
fn test_multiple_consecutive_line_endings() {
    let s = b"foo\n\nbar\n";
    let le = LineEnding::Byte(b'\n');
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"foo");
    assert_eq!(res[0].1, b"foo\n");
    assert_eq!(res[1].0, b"");
    assert_eq!(res[1].1, b"\n");
    assert_eq!(res[2].0, b"bar");
    assert_eq!(res[2].1, b"bar\n");
}

#[test]
#[should_panic(expected = "not allowing slower Finder search with one-byte ending")]
fn test_find_one_byte_with_finder() {
    let s = b"apple,banana,grape";
    let finder = Finder::new(b",");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let _res: Vec<_> = it.collect();
    // assert_eq!(res.len(), 3);
    // assert_eq!(res[0].0, b"apple");
    // assert_eq!(res[0].1, b"apple,");
    // assert_eq!(res[1].0, b"banana");
    // assert_eq!(res[1].1, b"banana,");
    // assert_eq!(res[2].0, b"grape");
    // assert_eq!(res[2].1, b"grape");
}

#[test]
fn test_find_multi_byte_with_finder() {
    let s = b"abc123def123ghi";
    let finder = Finder::new(b"123");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].0, b"abc");
    assert_eq!(res[0].1, b"abc123");
    assert_eq!(res[1].0, b"def");
    assert_eq!(res[1].1, b"def123");
    assert_eq!(res[2].0, b"ghi");
    assert_eq!(res[2].1, b"ghi");
}

#[test]
fn test_finder_trailing_sep() {
    let s = b"abc123";
    let finder = Finder::new(b"123");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"abc");
    assert_eq!(res[0].1, b"abc123");
}

#[test]
fn test_finder_no_match() {
    let s = b"abcdefgh";
    let finder = Finder::new(b"xyz");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"abcdefgh");
    assert_eq!(res[0].1, b"abcdefgh");
}

#[test]
fn test_finder_empty_input() {
    let s = b"";
    let finder = Finder::new(b"xyz");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, b"");
    assert_eq!(res[0].1, b"");
}

#[test]
fn test_finder_multiple_consecutive() {
    let s = b"xaaxaaxyzaaxyz";
    let finder = Finder::new(b"xyz");
    let le = LineEnding::MultiByte(finder);
    let it = line_ending_iter(s, &le);
    let res: Vec<_> = it.collect();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].0, b"xaaxaa");
    assert_eq!(res[0].1, b"xaaxaaxyz");
    assert_eq!(res[1].0, b"aa");
    assert_eq!(res[1].1, b"aaxyz");
}

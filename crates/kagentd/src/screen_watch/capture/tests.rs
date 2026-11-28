use super::*;

/// An `xwd` dump of a 2x1 window, 32 bits, little endian, red then blue.
fn dump() -> Vec<u8> {
    let name = b"w\0";
    let header = 100 + name.len();
    let mut words = vec![0u32; 25];
    words[0] = header as u32;
    words[1] = 7;
    words[2] = 2; // ZPixmap
    words[3] = 24;
    words[4] = 2;
    words[5] = 1;
    words[7] = 0; // LSBFirst
    words[11] = 32;
    words[12] = 8;
    words[13] = 4; // TrueColor
    words[14] = 0x00ff_0000;
    words[15] = 0x0000_ff00;
    words[16] = 0x0000_00ff;
    words[19] = 0;
    let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
    out.extend_from_slice(name);
    out.extend_from_slice(&0x00ff_0000u32.to_le_bytes());
    out.extend_from_slice(&0x0000_00ffu32.to_le_bytes());
    out
}

#[test]
fn an_xwd_dump_becomes_a_png() {
    let png = png_of_xwd(&dump()).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (2, 1));
    assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(img.get_pixel(1, 0).0, [0, 0, 255]);
}

#[test]
fn a_cut_or_odd_dump_is_an_error_not_a_panic() {
    let d = dump();
    assert!(png_of_xwd(&d[..d.len() - 3]).is_err());
    assert!(png_of_xwd(&d[..40]).is_err());
    assert!(png_of_xwd(&[]).is_err());
    let mut eight = d.clone();
    eight[11 * 4..12 * 4].copy_from_slice(&8u32.to_be_bytes());
    assert!(png_of_xwd(&eight).unwrap_err().contains("not supported"));
}

#[test]
fn window_ids_are_printed_as_hex() {
    assert_eq!(hex_id("73400330\n").as_deref(), Some("0x0460000a"));
    assert_eq!(hex_id("x"), None);
}

// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

//! Identities for the separately packaged OFL Noto font pack. No font bytes
//! or runtime loading are embedded here. Paths are relative to the explicit
//! trusted font-pack directory containing manifest.json.

#[derive(Clone, Copy, Debug)]
pub struct BundledFont {
    pub family: &'static str,
    pub path: &'static str,
    pub bytes: usize,
    pub sha256: [u8; 32],
}

pub const FONT_PACK_ID: &str = "tilewright-unicode-noto-v1";
pub const FONT_PACK_REVISION: &str = "6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec";
pub const TOTAL_FONT_BYTES: usize = 29197208;
pub const FONTS: &[BundledFont] = &[
    BundledFont {
        family: "Noto Sans",
        path: "fonts/notosans/NotoSans[wdth,wght].ttf",
        bytes: 2049096,
        sha256: [
            191, 183, 187, 105, 21, 19, 241, 46, 115, 77, 195, 70, 192, 58, 3, 247, 132, 145, 36,
            50, 215, 227, 250, 142, 86, 239, 207, 144, 111, 232, 107, 61,
        ],
    },
    BundledFont {
        family: "Noto Sans Arabic",
        path: "fonts/notosansarabic/NotoSansArabic[wdth,wght].ttf",
        bytes: 844676,
        sha256: [
            99, 17, 27, 91, 46, 7, 77, 212, 140, 198, 118, 146, 224, 162, 114, 109, 134, 238, 148,
            193, 195, 127, 232, 89, 130, 87, 183, 180, 232, 126, 134, 158,
        ],
    },
    BundledFont {
        family: "Noto Sans Hebrew",
        path: "fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf",
        bytes: 112640,
        sha256: [
            126, 243, 106, 44, 53, 147, 117, 140, 219, 98, 46, 27, 222, 244, 248, 69, 35, 233, 47,
            188, 60, 204, 102, 116, 56, 221, 128, 255, 84, 194, 222, 136,
        ],
    },
    BundledFont {
        family: "Noto Sans Devanagari",
        path: "fonts/notosansdevanagari/NotoSansDevanagari[wdth,wght].ttf",
        bytes: 641944,
        sha256: [
            20, 236, 74, 244, 31, 39, 72, 34, 22, 209, 194, 34, 159, 65, 127, 249, 177, 66, 94, 27,
            171, 176, 20, 229, 125, 29, 64, 208, 50, 41, 133, 62,
        ],
    },
    BundledFont {
        family: "Noto Sans Bengali",
        path: "fonts/notosansbengali/NotoSansBengali[wdth,wght].ttf",
        bytes: 463668,
        sha256: [
            220, 212, 41, 120, 9, 78, 88, 74, 132, 156, 132, 165, 20, 80, 238, 172, 64, 200, 130,
            96, 87, 213, 102, 234, 109, 75, 150, 39, 164, 3, 160, 90,
        ],
    },
    BundledFont {
        family: "Noto Sans Gujarati",
        path: "fonts/notosansgujarati/NotoSansGujarati[wdth,wght].ttf",
        bytes: 672904,
        sha256: [
            153, 1, 216, 85, 47, 29, 213, 210, 197, 13, 189, 76, 170, 111, 110, 23, 78, 116, 232,
            38, 79, 6, 89, 74, 178, 89, 174, 110, 123, 26, 196, 40,
        ],
    },
    BundledFont {
        family: "Noto Sans Gurmukhi",
        path: "fonts/notosansgurmukhi/NotoSansGurmukhi[wdth,wght].ttf",
        bytes: 268608,
        sha256: [
            30, 111, 114, 143, 166, 32, 229, 102, 248, 66, 216, 30, 34, 2, 101, 129, 63, 170, 18,
            119, 18, 20, 118, 93, 40, 156, 152, 224, 53, 173, 197, 242,
        ],
    },
    BundledFont {
        family: "Noto Sans Tamil",
        path: "fonts/notosanstamil/NotoSansTamil[wdth,wght].ttf",
        bytes: 340668,
        sha256: [
            170, 58, 155, 50, 31, 75, 11, 178, 196, 2, 3, 255, 189, 233, 175, 137, 113, 50, 39,
            134, 110, 14, 19, 247, 110, 91, 158, 238, 167, 39, 207, 136,
        ],
    },
    BundledFont {
        family: "Noto Sans Telugu",
        path: "fonts/notosanstelugu/NotoSansTelugu[wdth,wght].ttf",
        bytes: 888808,
        sha256: [
            230, 24, 175, 123, 249, 153, 223, 25, 46, 212, 243, 136, 235, 162, 229, 99, 242, 181,
            1, 80, 52, 233, 203, 179, 23, 181, 189, 121, 59, 215, 51, 77,
        ],
    },
    BundledFont {
        family: "Noto Sans Kannada",
        path: "fonts/notosanskannada/NotoSansKannada[wdth,wght].ttf",
        bytes: 641372,
        sha256: [
            204, 164, 243, 179, 168, 203, 18, 251, 38, 31, 27, 67, 186, 245, 210, 247, 245, 157,
            144, 254, 18, 61, 65, 240, 6, 94, 211, 161, 131, 153, 126, 201,
        ],
    },
    BundledFont {
        family: "Noto Sans Malayalam",
        path: "fonts/notosansmalayalam/NotoSansMalayalam[wdth,wght].ttf",
        bytes: 504204,
        sha256: [
            49, 46, 14, 124, 60, 193, 95, 160, 158, 180, 42, 143, 116, 158, 235, 36, 107, 89, 62,
            212, 32, 227, 200, 26, 175, 232, 217, 16, 195, 166, 251, 86,
        ],
    },
    BundledFont {
        family: "Noto Sans Oriya",
        path: "fonts/notosansoriya/NotoSansOriya[wdth,wght].ttf",
        bytes: 613116,
        sha256: [
            145, 3, 66, 39, 93, 97, 144, 129, 184, 150, 180, 237, 37, 253, 154, 214, 255, 50, 12,
            37, 89, 27, 129, 13, 225, 252, 196, 190, 177, 83, 186, 130,
        ],
    },
    BundledFont {
        family: "Noto Sans Sinhala",
        path: "fonts/notosanssinhala/NotoSansSinhala[wdth,wght].ttf",
        bytes: 1181956,
        sha256: [
            155, 217, 62, 64, 122, 39, 128, 117, 190, 64, 51, 36, 6, 59, 201, 74, 126, 48, 108, 68,
            222, 77, 248, 18, 20, 233, 50, 51, 12, 34, 238, 207,
        ],
    },
    BundledFont {
        family: "Noto Sans Thai",
        path: "fonts/notosansthai/NotoSansThai[wdth,wght].ttf",
        bytes: 218652,
        sha256: [
            90, 28, 85, 155, 181, 57, 88, 60, 138, 31, 217, 157, 28, 91, 148, 145, 229, 225, 68,
            120, 201, 205, 43, 208, 151, 13, 92, 48, 150, 204, 158, 248,
        ],
    },
    BundledFont {
        family: "Noto Sans SC",
        path: "fonts/notosanssc/NotoSansSC[wght].ttf",
        bytes: 17772300,
        sha256: [
            163, 4, 24, 17, 167, 140, 54, 27, 29, 229, 15, 149, 60, 128, 94, 2, 68, 149, 28, 33,
            197, 189, 65, 47, 114, 50, 239, 13, 137, 154, 240, 218,
        ],
    },
    BundledFont {
        family: "Noto Emoji",
        path: "fonts/notoemoji/NotoEmoji[wght].ttf",
        bytes: 1982596,
        sha256: [
            222, 108, 24, 131, 41, 56, 175, 201, 156, 175, 19, 43, 57, 214, 163, 10, 25, 186, 199,
            242, 232, 18, 226, 141, 178, 83, 91, 70, 8, 210, 117, 81,
        ],
    },
];

/// Exact manifest for the separately distributed pack; no font payload.
pub const FONT_PACK_MANIFEST: &str = include_str!("../assets/unicode-font-pack.json");
pub const FONT_PACK_MANIFEST_SHA256: [u8; 32] = [
    163, 50, 179, 93, 203, 40, 116, 248, 81, 117, 82, 169, 238, 110, 58, 101, 9, 230, 114, 141,
    253, 190, 113, 177, 239, 189, 226, 112, 6, 154, 162, 216,
];

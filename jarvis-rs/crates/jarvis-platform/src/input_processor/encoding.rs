/// Encode a key press into terminal escape sequences / bytes.
pub fn encode_key_for_terminal(key: &str, ctrl: bool, alt: bool, _shift: bool) -> Vec<u8> {
    let alt_prefix: &[u8] = if alt { b"\x1b" } else { b"" };

    match key {
        // Editing keys
        "Enter" => [alt_prefix, b"\r"].concat(),
        "Backspace" => [alt_prefix, b"\x7f"].concat(),
        "Tab" => [alt_prefix, b"\t"].concat(),
        "Escape" => [alt_prefix, b"\x1b"].concat(),
        "Space" => [alt_prefix, b" "].concat(),
        "Delete" => [alt_prefix, b"\x1b[3~"].concat(),
        "Insert" => [alt_prefix, b"\x1b[2~"].concat(),

        // Arrow keys
        "Up" => [alt_prefix, b"\x1b[A"].concat(),
        "Down" => [alt_prefix, b"\x1b[B"].concat(),
        "Right" => [alt_prefix, b"\x1b[C"].concat(),
        "Left" => [alt_prefix, b"\x1b[D"].concat(),

        // Navigation
        "Home" => [alt_prefix, b"\x1b[H"].concat(),
        "End" => [alt_prefix, b"\x1b[F"].concat(),
        "PageUp" => [alt_prefix, b"\x1b[5~"].concat(),
        "PageDown" => [alt_prefix, b"\x1b[6~"].concat(),

        // Function keys
        "F1" => [alt_prefix, b"\x1bOP"].concat(),
        "F2" => [alt_prefix, b"\x1bOQ"].concat(),
        "F3" => [alt_prefix, b"\x1bOR"].concat(),
        "F4" => [alt_prefix, b"\x1bOS"].concat(),
        "F5" => [alt_prefix, b"\x1b[15~"].concat(),
        "F6" => [alt_prefix, b"\x1b[17~"].concat(),
        "F7" => [alt_prefix, b"\x1b[18~"].concat(),
        "F8" => [alt_prefix, b"\x1b[19~"].concat(),
        "F9" => [alt_prefix, b"\x1b[20~"].concat(),
        "F10" => [alt_prefix, b"\x1b[21~"].concat(),
        "F11" => [alt_prefix, b"\x1b[23~"].concat(),
        "F12" => [alt_prefix, b"\x1b[24~"].concat(),

        _ => {
            if key.chars().count() == 1 {
                let ch = key.chars().next().unwrap();
                if ctrl && ch.is_ascii_alphabetic() {
                    let ctrl_byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    [alt_prefix, &[ctrl_byte]].concat()
                } else if ctrl && ch == '[' {
                    b"\x1b".to_vec()
                } else if ctrl && ch == '\\' {
                    vec![0x1c]
                } else if ctrl && ch == ']' {
                    vec![0x1d]
                } else {
                    [alt_prefix, key.as_bytes()].concat()
                }
            } else {
                Vec::new()
            }
        }
    }
}

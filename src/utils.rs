use regex::Regex;

pub fn get_host_port(uri: &String) -> String {
    return Regex::new(r"://(.*)/?")
        .unwrap()
        .captures(uri)
        .unwrap()
        .get(1)
        .unwrap()
        .as_str()
        .to_string();
}

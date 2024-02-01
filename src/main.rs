extern crate chrono;
use chrono::prelude::*;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};
use std::fs::{File};
use std::io::Write;
use windows::{
    core::{GUID,HSTRING,PCWSTR,PWSTR},
    Data::Xml::Dom::{XmlDocument, XmlElement},
    Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE, WIN32_ERROR},
        NetworkManagement::WiFi::{
            WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanGetProfile, WlanGetProfileList,
            WlanOpenHandle, WLAN_API_VERSION_2_0, WLAN_INTERFACE_INFO_LIST, WLAN_PROFILE_GET_PLAINTEXT_KEY,
            WLAN_PROFILE_INFO_LIST,
        },
    },
};

fn open_wlan_handle(api_version: u32) -> Result<HANDLE, windows::core::Error> {
    let mut negotiated_version = 0;
    let mut wlan_handle = INVALID_HANDLE_VALUE;
    let result = unsafe { WlanOpenHandle(api_version, None, &mut negotiated_version, &mut wlan_handle) };

    match WIN32_ERROR(result).ok() {
        Ok(_) => { return Ok(wlan_handle) }
        Err(e) => {
            let wlan_interface_unavailable_error = e.code().to_string() == String::from("0x80070426");

            eprintln!("Failed while opening handle: {:?}", e.message().to_string());
            let reason = if wlan_interface_unavailable_error {
                "WLAN interface is unavailable".to_string()
            } else {
                format!("error code {}", e.code().to_string()).to_string()
            };
            eprintln!("Reason: {:?}", reason);

            std::process::exit(1);
        }
    }
}

fn enum_wlan_interfaces(handle: HANDLE) -> Result<*mut WLAN_INTERFACE_INFO_LIST, windows::core::Error> {
    let mut interface_ptr = std::ptr::null_mut();

    let result = unsafe { WlanEnumInterfaces(handle, None, &mut interface_ptr) };

    WIN32_ERROR(result).ok()?;

    Ok(interface_ptr)
}

fn grab_interface_profiles(
    handle: HANDLE,
    interface_guid: &GUID,
) -> Result<*const WLAN_PROFILE_INFO_LIST, windows::core::Error> {
    let mut wlan_profiles_ptr = std::ptr::null_mut();

    let result = unsafe { WlanGetProfileList(handle, interface_guid, None, &mut wlan_profiles_ptr) };

    WIN32_ERROR(result).ok()?;

    Ok(wlan_profiles_ptr)
}

fn parse_utf16_slice(string_slice : &[u16]) -> Option<OsString> {
    let null_idx = string_slice.iter().position(|c| c == &0)?;

    Some(OsString::from_wide(&string_slice[..null_idx]))
}

fn load_xml_data(xml : &OsString) -> Result<XmlDocument, windows::core::Error> {
    let xml_document = XmlDocument::new()?;
    xml_document.LoadXml(&HSTRING::from(xml))?;
    Ok(xml_document)
}

fn traverse_xml_tree(xml: &XmlElement, node_path: &[&str]) -> Option<String> {
    let mut subtree_list = xml.ChildNodes().ok()?;
    let last_node_name = node_path.last()?;

    'node_traverse: for node in node_path {
       let node_name = OsString::from_wide(&node.encode_utf16().collect::<Vec<u16>>());

        for subtree_value in &subtree_list {
            let element_name = match subtree_value.NodeName() {
                Ok(name) => name,
                Err(_) => continue,
            };

            if element_name.to_os_string() == node_name {
                if element_name.to_os_string().to_string_lossy().to_string() == last_node_name.to_string() {
                    return Some(subtree_value.InnerText().ok()?.to_string());
                }

                subtree_list = subtree_value.ChildNodes().ok()?;
                continue 'node_traverse;
            }
        }
    }

    None
}

fn get_profile_xml(
    handle: HANDLE,
    interface_guid: &GUID,
    profile_name: &OsString,
) -> Result<OsString, windows::core::Error> {

    let mut profile_xml_data = PWSTR::null();
    let mut profile_flags = WLAN_PROFILE_GET_PLAINTEXT_KEY;

    let result = unsafe {WlanGetProfile(handle, interface_guid, PCWSTR(HSTRING::from(profile_name).as_ptr()), None, &mut profile_xml_data, Some(&mut profile_flags), None) };

    WIN32_ERROR(result).ok()?;

    let xml_string = match unsafe { profile_xml_data.to_hstring() } {
        Ok(data) => data,
        Err(e) => {
            unsafe { WlanFreeMemory(profile_xml_data.as_ptr().cast()) };
            return Err(e);
        }
    };

    Ok(xml_string.to_os_string())
}

enum SaveFileType {
    Txt,
    Json,
}

fn save_results(output: &[u8], file_name: &str, file_type: SaveFileType) -> std::io::Result<()> {
    let ext = match file_type {
        SaveFileType::Txt => "txt",
        SaveFileType::Json => "json"
    };

    let mut file = File::create(format!("{}.{}", file_name, ext))?;
    return match file.write(output) {
        Ok(_) => {
            println!("Saved output as {} file!", ext);
            Ok(())
        },
        Err(e) => {
            eprintln!("Failed to write output to a file. {:?}", e);
            Err(e)
        }
    }
}

fn main() -> std::io::Result<()> {
    let wlan_handle = open_wlan_handle(WLAN_API_VERSION_2_0).expect("Failed to open WLAN handle");

    let interface_ptr = match enum_wlan_interfaces(wlan_handle) {
        Ok(interfaces) => interfaces,
        Err(e) => {
            eprintln!("Failed to get wireless interfaces: {:?}", e);
            unsafe { WlanCloseHandle(wlan_handle, None) };
            std::process::exit(1);
        }
    };

    let interface_list = unsafe {
        std::slice::from_raw_parts(
            (*interface_ptr).InterfaceInfo.as_ptr(),
            (*interface_ptr).dwNumberOfItems as usize,
        )
    };

    for interface_info in interface_list {
        let _interface_description = match parse_utf16_slice(interface_info.strInterfaceDescription.as_slice()) {
            Some(name) => name,
            None => {
                eprintln!("Could not parse interface description");
                continue;
            }
        };

        let wlan_profile_ptr = match grab_interface_profiles(wlan_handle, &interface_info.InterfaceGuid) {
            Ok(profiles) => profiles,
            Err(_e) => {
                eprintln!("Failed to retrieve profiles");
                continue;
            }
        };

        let wlan_profile_list = unsafe { std::slice::from_raw_parts(
            (*wlan_profile_ptr).ProfileInfo.as_ptr(),
            (*wlan_profile_ptr).dwNumberOfItems as usize,
        ) };

        let mut output_json: String = String::from("[\n");
        let mut output: String = String::new();

        for profile in wlan_profile_list {
            let profile_name = match parse_utf16_slice(&profile.strProfileName) {
                Some(name) => name,
                None => {
                    eprintln!("Could not parse profile name");
                    continue;
                }
            };

            let profile_xml_data = match get_profile_xml(wlan_handle, &interface_info.InterfaceGuid, &profile_name) {
                Ok(data) => data,
                Err(_e) => {
                    eprintln!("Failed to extract XML data");
                    continue;
                }
            };

            let xml_document = match load_xml_data(&profile_xml_data) {
                Ok(data) => data,
                Err(_e) => {
                    eprintln!("Failed to extract XML document");
                    continue;
                }
            };

            let root = match xml_document.DocumentElement() {
                Ok(root) => root,
                Err(_e) => {
                    eprintln!("Failed to get document root for profile XML");
                    continue;
                }
            };

            let auth_type = match traverse_xml_tree(&root, &["MSM", "security", "authEncryption", "authentication"]) {
                Some(t) => t,
                None => {
                    eprintln!("Failed to get the auth type for this profile");
                    continue;
                }
            };

            let profile_name_string = profile_name.to_string_lossy().to_string();
            match auth_type.as_str() {
                "open" => {
                    output_json.push_str(format!("\t{{\"ssid\": {:?}, \"auth\": \"none\", \"password\":\"\"}},\n", profile_name_string).as_str());
                    output.push_str(format!("Wifi name: {:?}, No password", profile_name_string).as_str());
                },
                "WPA2" | "WPA2PSK" => {
                    if let Some(password) = traverse_xml_tree(&root, &["MSM", "security", "sharedKey", "keyMaterial"]) {
                        output_json.push_str(format!(
                            "\t{{\"ssid\": {:?}, \"auth\": {:?}, \"password\": {:?}}},\n",
                            profile_name_string,
                            auth_type,
                            password
                        ).as_str());
                        output.push_str(format!(
                            "Wifi name: {:?}, Authentication: {:?}, Password: {}\n",
                            profile_name_string,
                            auth_type,
                            password
                        ).as_str());
                    }
                }
                _ => {
                    if let Some(password) = traverse_xml_tree(&root, &["MSM", "security", "sharedKey", "keyMaterial"]) {
                        output_json.push_str(format!(
                            "\t{{\"ssid\": {:?}, \"auth\": {:?}, \"password\": {:?}}},\n",
                            profile_name_string,
                            auth_type,
                            password
                        ).as_str());
                        output.push_str(format!(
                            "Wifi name: {:?}, Authentication: {:?}, Password: {}\n",
                            profile_name_string,
                            auth_type,
                            password
                        ).as_str());
                    } else {
                        output_json.push_str(format!(
                            "\t{{\"ssid\": {:?}, \"auth\": {:?}}},\n",
                            profile_name_string,
                            auth_type
                        ).as_str());
                        output.push_str(format!(
                            "Wifi name: {:?}, Authentication: {:?}",
                            profile_name_string,
                            auth_type
                        ).as_str());
                    }
                }
            }
        }
        match output_json.rfind(',') {
            Some(idx) => { output_json.remove(idx) }
            None => { '\0' }
        };
        output_json.push(']');

        println!("{}", output);
        let date_as_string = Utc::now().format("%Y-%m-%d").to_string();

        save_results(&output.as_bytes(), format!("{}_WLAN_EXTRACTION", date_as_string).as_str(), SaveFileType::Txt)?;
        save_results(&output_json.as_bytes(), format!("{}_WLAN_EXTRACTION", date_as_string).as_str(), SaveFileType::Json)?;
    }

    unsafe { WlanFreeMemory(interface_ptr.cast()) };
    unsafe { WlanCloseHandle(wlan_handle, None) };

    Ok(())
}

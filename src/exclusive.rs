//! Hardware-level Exclusive DAC Mode & Automatic Hardware Sample-Rate Switching.
//!
//! - **macOS:** Direct `CoreAudio.framework` FFI implementing `kAudioDevicePropertyHogMode`
//!   (exclusive PID lock bypassing system mixing) and `kAudioDevicePropertyNominalSampleRate`
//!   (switches the physical USB/Thunderbolt/built-in DAC hardware clock to match the track).
//!   An RAII guard guarantees Hog Mode is always released and the original clock restored on drop.
//! - **Windows / Linux:** Graceful fallback with accurate status reporting while cpal negotiates
//!   the target hardware sample rate directly with WASAPI / ALSA / PipeWire.

/// Live status returned when acquiring hardware exclusive/clock access for an output stream.
#[derive(Debug, Clone, Default)]
pub struct ExclusiveStatus {
    pub hog_acquired: bool,
    pub hw_rate_switched: bool,
    #[allow(dead_code)]
    pub active_hw_rate: Option<u32>,
}

/// RAII guard holding an exclusive DAC lock and/or switched hardware sample rate.
/// Dropping the guard immediately releases exclusive ownership cleanly.
pub struct ExclusiveDeviceGuard {
    pub status: ExclusiveStatus,
    #[cfg(target_os = "macos")]
    device_id: Option<u32>,
    #[cfg(target_os = "macos")]
    prev_rate: Option<f64>,
}

impl ExclusiveDeviceGuard {
    pub fn acquire(
        device_name: &str,
        target_rate: u32,
        want_exclusive: bool,
        want_hw_rate_switch: bool,
    ) -> Self {
        #[cfg(target_os = "macos")]
        {
            macos::acquire_coreaudio(device_name, target_rate, want_exclusive, want_hw_rate_switch)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (device_name, target_rate, want_exclusive, want_hw_rate_switch);
            Self {
                status: ExclusiveStatus::default(),
            }
        }
    }
}

impl Drop for ExclusiveDeviceGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(dev_id) = self.device_id {
            if self.status.hog_acquired {
                macos::set_hog_mode(dev_id, -1);
            }
            if let Some(orig) = self.prev_rate {
                let _ = macos::set_nominal_sample_rate(dev_id, orig);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, c_char, c_void};
    use std::ptr;
    use std::thread;
    use std::time::Duration;

    use super::{ExclusiveDeviceGuard, ExclusiveStatus};

    type AudioObjectID = u32;
    type OSStatus = i32;

    const K_AUDIO_OBJECT_SYSTEM_OBJECT: AudioObjectID = 1;
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT: u32 = u32::from_be_bytes(*b"outp");
    const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

    const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = u32::from_be_bytes(*b"dev#");
    const K_AUDIO_OBJECT_PROPERTY_NAME: u32 = u32::from_be_bytes(*b"lnam");
    const K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE: u32 = u32::from_be_bytes(*b"nsrt");
    const K_AUDIO_DEVICE_PROPERTY_AVAILABLE_NOMINAL_SAMPLE_RATES: u32 =
        u32::from_be_bytes(*b"nsr#");
    const K_AUDIO_DEVICE_PROPERTY_HOG_MODE: u32 = u32::from_be_bytes(*b"oink");
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AudioValueRange {
        m_minimum: f64,
        m_maximum: f64,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            out_data_size: *mut u32,
        ) -> OSStatus;

        fn AudioObjectGetPropertyData(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            io_data_size: *mut u32,
            out_data: *mut c_void,
        ) -> OSStatus;

        fn AudioObjectSetPropertyData(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            in_data_size: u32,
            in_data: *const c_void,
        ) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(
            the_string: *const c_void,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    pub fn acquire_coreaudio(
        device_name: &str,
        target_rate: u32,
        want_exclusive: bool,
        want_hw_rate_switch: bool,
    ) -> ExclusiveDeviceGuard {
        let mut guard = ExclusiveDeviceGuard {
            status: ExclusiveStatus::default(),
            device_id: None,
            prev_rate: None,
        };
        if !want_exclusive && !want_hw_rate_switch {
            return guard;
        }
        let Some(dev_id) = find_device_by_name(device_name) else {
            return guard;
        };
        guard.device_id = Some(dev_id);

        if want_exclusive {
            let pid = std::process::id() as i32;
            if set_hog_mode(dev_id, pid) {
                guard.status.hog_acquired = true;
            }
        }

        if want_hw_rate_switch
            && let Some(cur_rate) = get_nominal_sample_rate(dev_id)
        {
            guard.status.active_hw_rate = Some(cur_rate.round() as u32);
            let want = target_rate as f64;
            if (cur_rate - want).abs() > 1.0 && device_supports_rate(dev_id, want) {
                if set_nominal_sample_rate(dev_id, want) {
                    // Allow USB/CoreAudio hardware PLL up to ~120ms to lock on the new clock
                    for _ in 0..8 {
                        if let Some(now) = get_nominal_sample_rate(dev_id)
                            && (now - want).abs() < 2.0
                        {
                            guard.status.hw_rate_switched = true;
                            guard.status.active_hw_rate = Some(target_rate);
                            guard.prev_rate = Some(cur_rate);
                            break;
                        }
                        thread::sleep(Duration::from_millis(15));
                    }
                }
            } else if (cur_rate - want).abs() <= 1.0 {
                guard.status.hw_rate_switched = true;
                guard.status.active_hw_rate = Some(target_rate);
            }
        }

        guard
    }

    fn find_device_by_name(wanted: &str) -> Option<AudioObjectID> {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut size: u32 = 0;
        let st = unsafe {
            AudioObjectGetPropertyDataSize(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &addr,
                0,
                ptr::null(),
                &mut size,
            )
        };
        if st != 0 || size == 0 {
            return None;
        }
        let count = size as usize / std::mem::size_of::<AudioObjectID>();
        let mut devs = vec![0u32; count];
        let st = unsafe {
            AudioObjectGetPropertyData(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &addr,
                0,
                ptr::null(),
                &mut size,
                devs.as_mut_ptr() as *mut c_void,
            )
        };
        if st != 0 {
            return None;
        }
        for dev in devs {
            if let Some(name) = get_device_name(dev)
                && name == wanted
            {
                return Some(dev);
            }
        }
        None
    }

    fn get_device_name(dev_id: AudioObjectID) -> Option<String> {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_OBJECT_PROPERTY_NAME,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut cf_str: *const c_void = ptr::null();
        let mut size = std::mem::size_of::<*const c_void>() as u32;
        let st = unsafe {
            AudioObjectGetPropertyData(
                dev_id,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut cf_str as *mut _ as *mut c_void,
            )
        };
        if st != 0 || cf_str.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        let ok = unsafe {
            CFStringGetCString(
                cf_str,
                buf.as_mut_ptr(),
                buf.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        unsafe { CFRelease(cf_str) };
        if ok == 0 {
            return None;
        }
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        Some(s)
    }

    fn get_nominal_sample_rate(dev_id: AudioObjectID) -> Option<f64> {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut rate: f64 = 0.0;
        let mut size = std::mem::size_of::<f64>() as u32;
        let st = unsafe {
            AudioObjectGetPropertyData(
                dev_id,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut rate as *mut f64 as *mut c_void,
            )
        };
        (st == 0 && rate > 0.0).then_some(rate)
    }

    fn device_supports_rate(dev_id: AudioObjectID, target: f64) -> bool {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_AVAILABLE_NOMINAL_SAMPLE_RATES,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut size: u32 = 0;
        let st = unsafe {
            AudioObjectGetPropertyDataSize(dev_id, &addr, 0, ptr::null(), &mut size)
        };
        if st != 0 || size == 0 {
            return false;
        }
        let count = size as usize / std::mem::size_of::<AudioValueRange>();
        let mut ranges = vec![
            AudioValueRange {
                m_minimum: 0.0,
                m_maximum: 0.0,
            };
            count
        ];
        let st = unsafe {
            AudioObjectGetPropertyData(
                dev_id,
                &addr,
                0,
                ptr::null(),
                &mut size,
                ranges.as_mut_ptr() as *mut c_void,
            )
        };
        if st != 0 {
            return false;
        }
        ranges
            .iter()
            .any(|r| target >= r.m_minimum - 1.0 && target <= r.m_maximum + 1.0)
    }

    pub fn set_nominal_sample_rate(dev_id: AudioObjectID, rate: f64) -> bool {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let st = unsafe {
            AudioObjectSetPropertyData(
                dev_id,
                &addr,
                0,
                ptr::null(),
                std::mem::size_of::<f64>() as u32,
                &rate as *const f64 as *const c_void,
            )
        };
        st == 0
    }

    pub fn set_hog_mode(dev_id: AudioObjectID, pid: i32) -> bool {
        let addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_HOG_MODE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_OUTPUT,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let st = unsafe {
            AudioObjectSetPropertyData(
                dev_id,
                &addr,
                0,
                ptr::null(),
                std::mem::size_of::<i32>() as u32,
                &pid as *const i32 as *const c_void,
            )
        };
        st == 0
    }
}

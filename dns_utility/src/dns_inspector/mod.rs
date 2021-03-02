// Copyright (c) 2019-2021, MASQ (https://masq.ai). All rights reserved.

#[cfg(target_os = "windows")]
extern crate winreg;

#[cfg(target_os = "macos")]
extern crate core_foundation;
#[cfg(target_os = "macos")]
extern crate system_configuration;

#[cfg(target_os = "windows")]
mod adapter_wrapper;
mod dns_modifier;
mod dns_modifier_factory;
mod dynamic_store_dns_modifier;
#[cfg(target_os = "windows")]
mod ipconfig_wrapper;
mod resolv_conf_dns_modifier;
#[cfg(target_os = "windows")]
mod win_dns_modifier;
mod utils;

use std::net::IpAddr;
use crate::dns_inspector::dns_modifier_factory::{DnsModifierFactoryReal, DnsModifierFactory};
use std::fmt::{Formatter, Debug};
use std::fmt;

#[derive (Clone, PartialEq)]
pub enum DnsInspectionError {
    NotConnected,
    BadEntryFormat(String),
    InvalidConfigFile(String),
}

impl Debug for DnsInspectionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DnsInspectionError::NotConnected => unimplemented!(), // This system does not appear to be connected to a network
            DnsInspectionError::BadEntryFormat(msg) => unimplemented!(), // Badly formatted nameserver line: {}
            DnsInspectionError::InvalidConfigFile(msg) => unimplemented!(), // /etc/resolv.conf is not a UTF-8 text file
        }
    }
}

pub fn dns_servers () -> Result<Vec<IpAddr>, DnsInspectionError> {
    let factory = DnsModifierFactoryReal::new();
    let modifier = factory.make().unwrap();
    modifier.inspect()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::dns_inspector::dns_modifier_factory::{DnsModifierFactoryReal, DnsModifierFactory};

    #[test]
    fn dns_servers_works() {
        let factory = DnsModifierFactoryReal::new();
        let modifier = factory.make().unwrap();
        let expected_result = modifier.inspect();

        let actual_result = dns_servers();

        assert_eq! (actual_result, expected_result);
    }
}
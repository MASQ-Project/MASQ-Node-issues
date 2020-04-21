// Copyright (c) 2019-2020, MASQ (https://masq.ai). All rights reserved.

use crate::blockchain::blockchain_interface::{chain_id_from_name, chain_name_from_id};
use crate::bootstrapper::BootstrapperConfig;
use crate::database::db_initializer::{DbInitializer, DbInitializerReal};
use crate::node_configurator::node_configurator_standard::standard::{
    privileged_parse_args, unprivileged_parse_args,
};
use crate::node_configurator::{app_head, data_directory_from_context, determine_config_file_path};
use crate::persistent_configuration::{PersistentConfiguration, PersistentConfigurationReal};
use crate::sub_lib::accountant::DEFAULT_EARNING_WALLET;
use crate::sub_lib::neighborhood::NodeDescriptor;
use crate::test_utils::main_cryptde;
use clap::value_t;
use itertools::Itertools;
use masq_lib::command::StdStreams;
use masq_lib::constants::DEFAULT_CHAIN_NAME;
use masq_lib::messages::UiSetupResponseValueStatus::{Blank, Configured, Default, Required, Set};
use masq_lib::messages::{UiSetupRequestValue, UiSetupResponseValue, UiSetupResponseValueStatus};
use masq_lib::multi_config::{
    CommandLineVcl, ConfigFileVcl, EnvironmentVcl, MultiConfig, VirtualCommandLine,
};
use masq_lib::shared_schema::{shared_app, ConfiguratorError};
use masq_lib::test_utils::fake_stream_holder::{ByteArrayReader, ByteArrayWriter};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

pub type SetupCluster = HashMap<String, UiSetupResponseValue>;

pub trait SetupReporter {
    fn get_modified_setup(
        &self,
        existing_setup: SetupCluster,
        incoming_setup: Vec<UiSetupRequestValue>,
    ) -> Result<SetupCluster, ConfiguratorError>;
}

pub struct SetupReporterReal {}

impl SetupReporter for SetupReporterReal {
    fn get_modified_setup(
        &self,
        mut existing_setup: SetupCluster,
        incoming_setup: Vec<UiSetupRequestValue>,
    ) -> Result<SetupCluster, ConfiguratorError> {
        let default_setup = Self::get_default_params();
        incoming_setup
            .iter()
            .filter(|v| v.value.is_none())
            .for_each(|v| {
                existing_setup.remove(&v.name);
            });
        let incoming_setup = incoming_setup
            .into_iter()
            .filter(|v| v.value.is_some())
            .map(|v| {
                (
                    v.name.clone(),
                    UiSetupResponseValue::new(&v.name, &v.value.expect("Value disappeared!"), Set),
                )
            })
            .collect::<SetupCluster>();
        let (real_user, data_directory_opt, chain_name) =
            Self::calculate_fundamentals(Self::combine_clusters(vec![
                &default_setup,
                &existing_setup,
                &incoming_setup,
            ]))?;
        let data_directory =
            data_directory_from_context(&real_user, &data_directory_opt, &chain_name);
eprintln_setup ("DEFAULT", &default_setup);
eprintln_setup ("EXISTING", &existing_setup);
eprintln_setup ("INCOMING", &incoming_setup);
        let combined_setup = Self::combine_clusters(vec![
            &default_setup,
            &existing_setup,
            &incoming_setup,
        ]);
eprintln_setup ("FOR USE WITH calculate_configured_setup", &combined_setup);
        let configured_setup = Self::calculate_configured_setup(
            combined_setup,
            &data_directory,
            &chain_name,
        )?;

eprintln_setup ("CONFIGURED", &configured_setup);
        let combined_setup = Self::combine_clusters(vec![
            &default_setup,
            &configured_setup,
            &existing_setup,
            &incoming_setup,
        ]);
eprintln_setup ("FOR USE WITH FINAL RUN THROUGH RETRIEVERS", &combined_setup);
        Ok(value_retrievers()
            .into_iter()
            .map(|retriever| {
                let make_blank_or_required = || {
                    let status = if retriever.is_required(&combined_setup) {
                        Required
                    } else {
                        Blank
                    };
                    (
                        retriever.value_name().to_string(),
                        UiSetupResponseValue::new(retriever.value_name(), "", status),
                    )
                };
                match combined_setup.get(retriever.value_name()) {
                    Some(uisrv) if vec![Blank, Required].contains(&uisrv.status) => {
                        make_blank_or_required()
                    }
                    Some(uisrv) => (retriever.value_name().to_string(), uisrv.clone()),
                    None => make_blank_or_required(),
                }
            })
            .collect::<SetupCluster>())
    }
}

fn eprintln_setup(label: &str, cluster: &SetupCluster) {
    let message = cluster.iter()
        .map(|(_, v)| (v.name.to_string(), v.value.to_string(), v.status))
        .sorted_by_key (|(n, _, _)| n.clone())
        .map(|(n, v, s)| format!("{:26}{:65}{:?}", n, v, s))
        .join("\n");
    eprintln! ("{}:\n{}\n", label, message);
}

impl SetupReporterReal {
    pub fn new() -> Self {
        Self {}
    }

    pub fn get_default_params() -> SetupCluster {
        let schema = shared_app(app_head());
        schema
            .p
            .opts
            .iter()
            .flat_map(|opt| {
                let name = opt.b.name;
                match opt.v.default_val {
                    Some(os_str) => {
                        let value = match os_str.to_str() {
                            Some(v) => v,
                            None => unimplemented!(),
                        };
                        Some((
                            name.to_string(),
                            UiSetupResponseValue::new(name, value, Default),
                        ))
                    }
                    None => None,
                }
            })
            .collect()
    }

    fn real_user_from_str(s: &str) -> crate::bootstrapper::RealUser {
        crate::bootstrapper::RealUser::from_str(s).expect("real-user validator is buggy")
    }

    fn calculate_fundamentals(
        combined_setup: SetupCluster,
    ) -> Result<(crate::bootstrapper::RealUser, Option<PathBuf>, String), ConfiguratorError> {
eprintln! ("Calculating fundamentals");
        let multi_config = Self::make_multi_config(None, true, false)?;
        let real_user = match (
            value_m!(multi_config, "real-user", String),
            combined_setup.get("real-user"),
        ) {
            (Some(real_user_str), None) => Self::real_user_from_str(&real_user_str),
            (Some(_), Some(uisrv)) if uisrv.status == Set => Self::real_user_from_str(&uisrv.value),
            (Some(real_user_str), Some(_)) => Self::real_user_from_str(&real_user_str),
            (None, Some(uisrv)) => Self::real_user_from_str(&uisrv.value),
            (None, None) => crate::bootstrapper::RealUser::null().populate(),
        };
        let data_directory_opt = match (
            value_m!(multi_config, "data-directory", String),
            combined_setup.get("data-directory"),
        ) {
            (Some(ddir_str), None) => Some(PathBuf::from(&ddir_str)),
            (Some(_), Some(uisrv)) if uisrv.status == Set => Some(PathBuf::from(&uisrv.value)),
            (Some(ddir_str), Some(_)) => Some(PathBuf::from(&ddir_str)),
            (None, Some(uisrv)) => Some(PathBuf::from(&uisrv.value)),
            (None, None) => None,
        };
        let chain_name = match (
            value_m!(multi_config, "chain", String),
            combined_setup.get("chain"),
        ) {
            (Some(chain_str), None) => chain_str,
            (Some(_), Some(uisrv)) if uisrv.status == Set => uisrv.value.clone(),
            (Some(chain_str), Some(_)) => chain_str,
            (None, Some(uisrv)) => uisrv.value.clone(),
            (None, None) => DEFAULT_CHAIN_NAME.to_string(),
        };
        Ok((real_user, data_directory_opt, chain_name))
    }

    fn calculate_configured_setup(
        combined_setup: SetupCluster,
        data_directory: &PathBuf,
        chain_name: &str,
    ) -> Result<SetupCluster, ConfiguratorError> {
        let db_password_opt = combined_setup.get("db-password").map(|v| v.value.clone());
        let command_line = Self::make_command_line(combined_setup);
eprintln! ("Calculating configured setup");
        let multi_config = Self::make_multi_config(Some(command_line), true, true)?;
        let (bootstrapper_config, persistent_config_opt) = Self::run_configuration(
            &multi_config,
            data_directory,
            chain_id_from_name(chain_name),
        )?;
        let mut setup = value_retrievers()
            .into_iter()
            .map(|r| {
                let computed_default = r.computed_default_value(
                    &bootstrapper_config,
                    &persistent_config_opt,
                    &db_password_opt,
                );
                let configured = match value_m!(multi_config, r.value_name(), String) {
                    Some(value) => UiSetupResponseValue::new(r.value_name(), &value, Configured),
                    None => UiSetupResponseValue::new(r.value_name(), "", Blank),
                };
                (
                    r.value_name().to_string(),
                    Self::choose_uisrv(&computed_default, &configured).clone(),
                )
            })
            .collect::<SetupCluster>();
        match setup.get_mut("config-file") {
            // special case because of early processing
            Some(uisrv) if &uisrv.value == "config.toml" => uisrv.status = Default,
            _ => (),
        };
        Ok(setup)
    }

    fn combine_clusters(clusters: Vec<&SetupCluster>) -> SetupCluster {
        let mut result: SetupCluster = HashMap::new();
        clusters.into_iter().for_each(|cluster| {
            let mut step: SetupCluster = HashMap::new();
            cluster.iter().for_each(|(k, incoming)| {
                match result.get(k) {
                    Some(existing) => {
                        step.insert(k.clone(), Self::choose_uisrv(existing, incoming).clone())
                    }
                    None => step.insert(k.clone(), incoming.clone()),
                };
            });
            result.extend(step);
        });
        result
    }

    fn choose_uisrv<'a>(
        existing: &'a UiSetupResponseValue,
        incoming: &'a UiSetupResponseValue,
    ) -> &'a UiSetupResponseValue {
        if incoming.status.value() >= existing.status.value() {
            incoming
        } else {
            existing
        }
    }

    fn make_command_line(setup: SetupCluster) -> Vec<String> {
        let accepted_statuses = vec![Set, Configured];
        let mut command_line = setup
            .into_iter()
            .filter(|(_, v)| accepted_statuses.contains(&v.status))
            .flat_map(|(_, v)| vec![format!("--{}", v.name), v.value])
            .collect::<Vec<String>>();
        command_line.insert(0, "program_name".to_string());
        command_line
    }

    fn make_multi_config<'a>(
        command_line_opt: Option<Vec<String>>,
        environment: bool,
        config_file: bool,
    ) -> Result<MultiConfig<'a>, ConfiguratorError> {
        let app = shared_app(app_head());
        let mut vcls: Vec<Box<dyn VirtualCommandLine>> = vec![];
        if let Some(command_line) = command_line_opt.clone() {
            vcls.push(Box::new(CommandLineVcl::new(command_line)));
        }
        if environment {
            vcls.push(Box::new(EnvironmentVcl::new(&app)));
        }
        if config_file {
            let command_line = match command_line_opt {
                Some(command_line) => command_line,
                None => vec![],
            };
            let (config_file_path, user_specified) =
                determine_config_file_path(&app, &command_line)?;
            vcls.push(Box::new(ConfigFileVcl::new(
                &config_file_path,
                user_specified,
            )));
        }
        MultiConfig::try_new(&app, vcls)
    }

    fn run_configuration(
        multi_config: &MultiConfig,
        data_directory: &PathBuf,
        chain_id: u8,
    ) -> Result<(BootstrapperConfig, Option<Box<dyn PersistentConfiguration>>), ConfiguratorError>
    {
        let mut streams = StdStreams {
            stdin: &mut ByteArrayReader::new(b""),
            stdout: &mut ByteArrayWriter::new(),
            stderr: &mut ByteArrayWriter::new(),
        };
        let mut bootstrapper_config = BootstrapperConfig::new();
        privileged_parse_args(multi_config, &mut bootstrapper_config, &mut streams)?;
        let initializer = DbInitializerReal::new();
        match initializer.initialize(data_directory, chain_id, false) {
            Ok(conn) => {
                let persistent_config = PersistentConfigurationReal::from(conn);
                unprivileged_parse_args(
                    multi_config,
                    &mut bootstrapper_config,
                    &mut streams,
                    Some(&persistent_config),
                )?;
                Ok((bootstrapper_config, Some(Box::new(persistent_config))))
            }
            Err(_) => {
                unprivileged_parse_args(
                    multi_config,
                    &mut bootstrapper_config,
                    &mut streams,
                    None,
                )?;
                Ok((bootstrapper_config, None))
            }
        }
    }
}

trait ValueRetriever {
    fn value_name(&self) -> &'static str;

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        None
    }

    fn computed_default_value(
        &self,
        bootstrapper_config: &BootstrapperConfig,
        persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        db_password_opt: &Option<String>,
    ) -> UiSetupResponseValue {
        match self.computed_default(bootstrapper_config, persistent_config_opt, db_password_opt) {
            Some((value, status)) => UiSetupResponseValue::new(self.value_name(), &value, status),
            None => UiSetupResponseValue::new(self.value_name(), "", Blank),
        }
    }

    fn set_value(&self, multi_config: &MultiConfig) -> Option<String> {
        value_m!(multi_config, self.value_name(), String)
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        false
    }
}

fn is_required_for_blockchain(params: &SetupCluster) -> bool {
    match params.get("neighborhood-mode") {
        Some(nhm) if &nhm.value == "zero-hop" => false,
        _ => true,
    }
}

struct BlockchainServiceUrl {}
impl ValueRetriever for BlockchainServiceUrl {
    fn value_name(&self) -> &'static str {
        "blockchain-service-url"
    }

    fn is_required(&self, params: &SetupCluster) -> bool {
        is_required_for_blockchain(params)
    }
}

struct Chain {}
impl ValueRetriever for Chain {
    fn value_name(&self) -> &'static str {
        "chain"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        Some((DEFAULT_CHAIN_NAME.to_string(), Default))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

struct ClandestinePort {}
impl ValueRetriever for ClandestinePort {
    fn value_name(&self) -> &'static str {
        "clandestine-port"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        persistent_config_opt
            .as_ref()
            .map(|pc| (pc.clandestine_port().to_string(), Configured))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

struct ConfigFile {}
impl ValueRetriever for ConfigFile {
    fn value_name(&self) -> &'static str {
        "config-file"
    }
}

struct ConsumingPrivateKey {}
impl ValueRetriever for ConsumingPrivateKey {
    fn value_name(&self) -> &'static str {
        "consuming-private-key"
    }
}

struct DataDirectory {}
impl ValueRetriever for DataDirectory {
    fn value_name(&self) -> &'static str {
        "data-directory"
    }

    fn computed_default(
        &self,
        bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        let real_user = &bootstrapper_config.real_user;
        let chain_name = chain_name_from_id(bootstrapper_config.blockchain_bridge_config.chain_id);
        let data_directory_opt = None;
        Some((
            data_directory_from_context(&real_user, &data_directory_opt, chain_name)
                .to_string_lossy()
                .to_string(),
            Default,
        ))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

struct DbPassword {}
impl ValueRetriever for DbPassword {
    fn value_name(&self) -> &'static str {
        "db-password"
    }

    fn is_required(&self, params: &SetupCluster) -> bool {
        is_required_for_blockchain(params)
    }
}

struct DnsServers {}
impl ValueRetriever for DnsServers {
    fn value_name(&self) -> &'static str {
        "dns-servers"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        Some(("1.1.1.1".to_string(), Default))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

struct EarningWallet {}
impl ValueRetriever for EarningWallet {
    fn value_name(&self) -> &'static str {
        "earning-wallet"
    }

    fn computed_default(
        &self,
        bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        let configured_wallet = &bootstrapper_config.earning_wallet;
        if configured_wallet.address() == DEFAULT_EARNING_WALLET.address() {
            Some((DEFAULT_EARNING_WALLET.to_string(), Default))
        } else {
            Some((configured_wallet.to_string(), Configured))
        }
    }

    fn is_required(&self, params: &SetupCluster) -> bool {
        is_required_for_blockchain(params)
    }
}

struct GasPrice {}
impl ValueRetriever for GasPrice {
    fn value_name(&self) -> &'static str {
        "gas-price"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        persistent_config_opt
            .as_ref()
            .map(|pc| (pc.gas_price().to_string(), Configured))
    }

    fn is_required(&self, params: &SetupCluster) -> bool {
        is_required_for_blockchain(params)
    }
}

struct Ip {}
impl ValueRetriever for Ip {
    fn value_name(&self) -> &'static str {
        "ip"
    }

    fn is_required(&self, params: &SetupCluster) -> bool {
        match params.get("neighborhood-mode") {
            Some(nhm) if &nhm.value == "standard" => true,
            Some(_) => false,
            None => true,
        }
    }
}

struct LogLevel {}
impl ValueRetriever for LogLevel {
    fn value_name(&self) -> &'static str {
        "log-level"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        Some(("warn".to_string(), Default))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

struct NeighborhoodMode {}
impl ValueRetriever for NeighborhoodMode {
    fn value_name(&self) -> &'static str {
        "neighborhood-mode"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        Some(("standard".to_string(), Default))
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        true
    }
}

fn node_descriptors_to_neighbors(node_descriptors: Vec<NodeDescriptor>) -> String {
    node_descriptors
        .into_iter()
        .map(|nd| nd.to_string(main_cryptde()))
        .collect_vec()
        .join(",")
}

struct Neighbors {}
impl ValueRetriever for Neighbors {
    fn value_name(&self) -> &'static str {
        "neighbors"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        match (persistent_config_opt, db_password_opt) {
            (Some(pc), Some(pw)) => match pc.past_neighbors(&pw) {
                Ok(Some(pns)) => Some((node_descriptors_to_neighbors(pns), Configured)),
                _ => None,
            },
            _ => None,
        }
    }

    fn is_required(&self, _params: &SetupCluster) -> bool {
        match _params.get("neighborhood-mode") {
            Some(nhm) if &nhm.value == "standard" => false,
            Some(nhm) if &nhm.value == "zero-hop" => false,
            _ => true,
        }
    }
}

struct RealUser {}
impl ValueRetriever for RealUser {
    fn value_name(&self) -> &'static str {
        "real-user"
    }

    fn computed_default(
        &self,
        _bootstrapper_config: &BootstrapperConfig,
        _persistent_config_opt: &Option<Box<dyn PersistentConfiguration>>,
        _db_password_opt: &Option<String>,
    ) -> Option<(String, UiSetupResponseValueStatus)> {
        #[cfg(target_os = "windows")]
        {
            None
        }
        #[cfg(not(target_os = "windows"))]
        {
            Some((
                crate::bootstrapper::RealUser::default()
                    .populate()
                    .to_string(),
                Default,
            ))
        }
    }
}

fn value_retrievers() -> Vec<Box<dyn ValueRetriever>> {
    vec![
        Box::new(BlockchainServiceUrl {}),
        Box::new(Chain {}),
        Box::new(ClandestinePort {}),
        Box::new(ConfigFile {}),
        Box::new(ConsumingPrivateKey {}),
        Box::new(DataDirectory {}),
        Box::new(DbPassword {}),
        Box::new(DnsServers {}),
        Box::new(EarningWallet {}),
        Box::new(GasPrice {}),
        Box::new(Ip {}),
        Box::new(LogLevel {}),
        Box::new(NeighborhoodMode {}),
        Box::new(Neighbors {}),
        Box::new(RealUser {}),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockchain::blockchain_interface::chain_id_from_name;
    use crate::bootstrapper::RealUser;
    use crate::database::db_initializer::{DbInitializer, DbInitializerReal};
    use crate::persistent_configuration::{
        PersistentConfigError, PersistentConfiguration, PersistentConfigurationReal,
    };
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::node_addr::NodeAddr;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::persistent_configuration_mock::PersistentConfigurationMock;
    use masq_lib::messages::UiSetupResponseValueStatus;
    use masq_lib::messages::UiSetupResponseValueStatus::{Blank, Configured, Required, Set};
    use masq_lib::test_utils::environment_guard::EnvironmentGuard;
    use masq_lib::test_utils::utils::ensure_node_home_directory_exists;
    #[cfg(not(target_os = "windows"))]
    use std::default::Default;
    use std::net::IpAddr;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    #[test]
    fn everything_in_defaults_is_properly_constructed() {
        let result = SetupReporterReal::get_default_params();

        assert_eq!(result.is_empty(), false, "{:?}", result); // if we don't have any defaults, let's get rid of all this
        result.into_iter().for_each(|(name, value)| {
            assert_eq!(name, value.name);
            assert_eq!(value.status, Default);
        });
    }

    #[test]
    fn some_items_are_censored_from_defaults() {
        let result = SetupReporterReal::get_default_params();

        assert_eq!(result.get("ui-port"), None, "{:?}", result);
        #[cfg(target_os = "windows")]
        assert_eq!(result.get("real-user"), None, "{:?}", result);
    }

    #[test]
    fn get_modified_setup_database_populated_only_requireds_set() {
        let _guard = EnvironmentGuard::new();
        let home_dir = ensure_node_home_directory_exists(
            "setup_reporter",
            "get_modified_setup_database_populated_only_requireds_set",
        );
        let db_initializer = DbInitializerReal::new();
        let conn = db_initializer
            .initialize(&home_dir, chain_id_from_name("mainnet"), true)
            .unwrap();
        let config = PersistentConfigurationReal::from(conn);
        config.set_password("password");
        config.set_clandestine_port(1234);
        config
            .set_mnemonic_seed(b"booga booga", "password")
            .unwrap();
        config.set_consuming_wallet_derivation_path("m/44'/60'/1'/2/3", "password");
        config.set_earning_wallet_address("0x0000000000000000000000000000000000000000");
        config.set_gas_price(1234567890);
        let neighbor1 = NodeDescriptor {
            encryption_public_key: PublicKey::new(b"ABCD"),
            mainnet: true,
            node_addr_opt: Some(NodeAddr::new(
                &IpAddr::from_str("1.2.3.4").unwrap(),
                &vec![1234],
            )),
        };
        let neighbor2 = NodeDescriptor {
            encryption_public_key: PublicKey::new(b"EFGH"),
            mainnet: true,
            node_addr_opt: Some(NodeAddr::new(
                &IpAddr::from_str("5.6.7.8").unwrap(),
                &vec![5678],
            )),
        };
        config
            .set_past_neighbors(Some(vec![neighbor1, neighbor2]), "password")
            .unwrap();

        let incoming_setup = vec![
            ("data-directory", home_dir.to_str().unwrap()),
            ("db-password", "password"),
            ("ip", "4.3.2.1"),
        ]
        .into_iter()
        .map(|(name, value)| UiSetupRequestValue::new(name, value))
        .collect_vec();
        let subject = SetupReporterReal::new();

        let result = subject
            .get_modified_setup(HashMap::new(), incoming_setup)
            .unwrap();

        let expected_result = vec![
            ("blockchain-service-url", "", Required),
            ("chain", "mainnet", Default),
            ("clandestine-port", "1234", Configured),
            ("config-file", "config.toml", Default),
            ("consuming-private-key", "", Blank),
            ("data-directory", home_dir.to_str().unwrap(), Set),
            ("db-password", "password", Set),
            ("dns-servers", "1.1.1.1", Default),
            (
                "earning-wallet",
                "0x0000000000000000000000000000000000000000",
                Configured,
            ),
            ("gas-price", "1234567890", Configured),
            ("ip", "4.3.2.1", Set),
            ("log-level", "warn", Default),
            ("neighborhood-mode", "standard", Default),
            (
                "neighbors",
                "QUJDRA@1.2.3.4:1234,RUZHSA@5.6.7.8:5678",
                Configured,
            ),
            #[cfg(not(target_os = "windows"))]
            (
                "real-user",
                &RealUser::default().populate().to_string(),
                Default,
            ),
        ]
        .into_iter()
        .map(|(name, value, status)| {
            (
                name.to_string(),
                UiSetupResponseValue::new(name, value, status),
            )
        })
        .collect_vec();
        let presentable_result = result
            .into_iter()
            .sorted_by_key(|(k, _)| k.clone())
            .collect_vec();
        assert_eq!(presentable_result, expected_result);
    }

    #[test]
    fn get_modified_setup_database_nonexistent_everything_preexistent() {
        let _guard = EnvironmentGuard::new();
        let home_dir = ensure_node_home_directory_exists(
            "setup_reporter",
            "get_modified_setup_database_nonexistent_everything_preexistent",
        );
        let existing_setup = vec![
            ("blockchain-service-url", "https://example.com"),
            ("chain", "ropsten"),
            ("clandestine-port", "1234"),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677"),
            ("data-directory", home_dir.to_str().unwrap()),
            ("db-password", "password"),
            ("dns-servers", "8.8.8.8"),
            ("earning-wallet", "0x0123456789012345678901234567890123456789"),
            ("gas-price", "50"),
            ("ip", "4.3.2.1"),
            ("log-level", "error"),
            ("neighborhood-mode", "originate-only"),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678"),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga"),
        ].into_iter()
            .map (|(name, value)| (name.to_string(), UiSetupResponseValue::new(name, value, Set)))
            .collect::<SetupCluster>();
        let subject = SetupReporterReal::new();

        let result = subject.get_modified_setup(existing_setup, vec![]).unwrap();

        let expected_result = vec![
            ("blockchain-service-url", "https://example.com", Set),
            ("chain", "ropsten", Set),
            ("clandestine-port", "1234", Set),
            ("config-file", "config.toml", Default),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677", Set),
            ("data-directory", home_dir.to_str().unwrap(), Set),
            ("db-password", "password", Set),
            ("dns-servers", "8.8.8.8", Set),
            ("earning-wallet", "0x0123456789012345678901234567890123456789", Set),
            ("gas-price", "50", Set),
            ("ip", "4.3.2.1", Set),
            ("log-level", "error", Set),
            ("neighborhood-mode", "originate-only", Set),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678", Set),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga", Set),
        ].into_iter()
            .map (|(name, value, status)| (name.to_string(), UiSetupResponseValue::new(name, value, status)))
            .collect_vec();
        let presentable_result = result
            .into_iter()
            .sorted_by_key(|(k, _)| k.clone())
            .collect_vec();
        assert_eq!(presentable_result, expected_result);
    }

    #[test]
    fn get_modified_setup_database_nonexistent_everything_set() {
        let _guard = EnvironmentGuard::new();
        let home_dir = ensure_node_home_directory_exists(
            "setup_reporter",
            "get_modified_setup_database_nonexistent_everything_set",
        );
        let incoming_setup = vec![
            ("blockchain-service-url", "https://example.com"),
            ("chain", "ropsten"),
            ("clandestine-port", "1234"),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677"),
            ("data-directory", home_dir.to_str().unwrap()),
            ("db-password", "password"),
            ("dns-servers", "8.8.8.8"),
            ("earning-wallet", "0x0123456789012345678901234567890123456789"),
            ("gas-price", "50"),
            ("ip", "4.3.2.1"),
            ("log-level", "error"),
            ("neighborhood-mode", "originate-only"),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678"),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga"),
        ].into_iter()
            .map (|(name, value)| UiSetupRequestValue::new(name, value))
            .collect_vec();
        let subject = SetupReporterReal::new();

        let result = subject
            .get_modified_setup(HashMap::new(), incoming_setup)
            .unwrap();

        let expected_result = vec![
            ("blockchain-service-url", "https://example.com", Set),
            ("chain", "ropsten", Set),
            ("clandestine-port", "1234", Set),
            ("config-file", "config.toml", Default),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677", Set),
            ("data-directory", home_dir.to_str().unwrap(), Set),
            ("db-password", "password", Set),
            ("dns-servers", "8.8.8.8", Set),
            ("earning-wallet", "0x0123456789012345678901234567890123456789", Set),
            ("gas-price", "50", Set),
            ("ip", "4.3.2.1", Set),
            ("log-level", "error", Set),
            ("neighborhood-mode", "originate-only", Set),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678", Set),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga", Set),
        ].into_iter()
            .map (|(name, value, status)| (name.to_string(), UiSetupResponseValue::new(name, value, status)))
            .collect_vec();
        let presentable_result = result
            .into_iter()
            .sorted_by_key(|(k, _)| k.clone())
            .collect_vec();
        assert_eq!(presentable_result, expected_result);
    }

    #[test]
    fn get_modified_setup_database_nonexistent_nothing_set_everything_in_environment() {
        let _guard = EnvironmentGuard::new();
        let home_dir = ensure_node_home_directory_exists(
            "setup_reporter",
            "get_modified_setup_database_nonexistent_nothing_set_everything_in_environment",
        );
        vec![
            ("SUB_BLOCKCHAIN_SERVICE_URL", "https://example.com"),
            ("SUB_CHAIN", "ropsten"),
            ("SUB_CLANDESTINE_PORT", "1234"),
            ("SUB_CONSUMING_PRIVATE_KEY", "0011223344556677001122334455667700112233445566770011223344556677"),
            ("SUB_DATA_DIRECTORY", home_dir.to_str().unwrap()),
            ("SUB_DB_PASSWORD", "password"),
            ("SUB_DNS_SERVERS", "8.8.8.8"),
            ("SUB_EARNING_WALLET", "0x0123456789012345678901234567890123456789"),
            ("SUB_GAS_PRICE", "50"),
            ("SUB_IP", "4.3.2.1"),
            ("SUB_LOG_LEVEL", "error"),
            ("SUB_NEIGHBORHOOD_MODE", "originate-only"),
            ("SUB_NEIGHBORS", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678"),
            #[cfg(not(target_os = "windows"))]
            ("SUB_REAL_USER", "9999:9999:booga"),
        ].into_iter()
            .for_each (|(name, value)| std::env::set_var (name, value));
        let params = vec![];
        let subject = SetupReporterReal::new();

        let result = subject.get_modified_setup(HashMap::new(), params).unwrap();

        let expected_result = vec![
            ("blockchain-service-url", "https://example.com", Configured),
            ("chain", "ropsten", Configured),
            ("clandestine-port", "1234", Configured),
            ("config-file", "config.toml", Default),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677", Configured),
            ("data-directory", home_dir.to_str().unwrap(), Configured),
            ("db-password", "password", Configured),
            ("dns-servers", "8.8.8.8", Configured),
            ("earning-wallet", "0x0123456789012345678901234567890123456789", Configured),
            ("gas-price", "50", Configured),
            ("ip", "4.3.2.1", Configured),
            ("log-level", "error", Configured),
            ("neighborhood-mode", "originate-only", Configured),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678", Configured),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga", Configured),
        ].into_iter()
            .map (|(name, value, status)| (name.to_string(), UiSetupResponseValue::new(name, value, status)))
            .collect_vec();
        let presentable_result = result
            .into_iter()
            .sorted_by_key(|(k, _)| k.clone())
            .collect_vec();
        assert_eq!(presentable_result, expected_result);
    }

    #[test]
    fn get_modified_setup_database_nonexistent_all_but_requireds_cleared() {
        let _guard = EnvironmentGuard::new();
        let home_dir = ensure_node_home_directory_exists(
            "setup_reporter",
            "get_modified_setup_database_nonexistent_all_but_requireds_cleared",
        );
        vec![
            ("SUB_BLOCKCHAIN_SERVICE_URL", "https://example.com"),
            ("SUB_CHAIN", "ropsten"),
            ("SUB_CLANDESTINE_PORT", "1234"),
            ("SUB_CONSUMING_PRIVATE_KEY", "0011223344556677001122334455667700112233445566770011223344556677"),
            ("SUB_DATA_DIRECTORY", home_dir.to_str().unwrap()),
            ("SUB_DB_PASSWORD", "password"),
            ("SUB_DNS_SERVERS", "8.8.8.8"),
            ("SUB_EARNING_WALLET", "0x0123456789012345678901234567890123456789"),
            ("SUB_GAS_PRICE", "50"),
            ("SUB_IP", "4.3.2.1"),
            ("SUB_LOG_LEVEL", "error"),
            ("SUB_NEIGHBORHOOD_MODE", "originate-only"),
            ("SUB_NEIGHBORS", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678"),
            #[cfg(not(target_os = "windows"))]
            ("SUB_REAL_USER", "9999:9999:booga"),
        ].into_iter()
            .for_each (|(name, value)| std::env::set_var (name, value));
        let params = vec![
            "blockchain-service-url",
            "clandestine-port",
            "config-file",
            "consuming-private-key",
            "data-directory",
            "db-password",
            "dns-servers",
            "earning-wallet",
            "gas-price",
            "ip",
            "neighborhood-mode",
            "neighbors",
            #[cfg(not(target_os = "windows"))]
            "real-user",
        ]
        .into_iter()
        .map(|name| UiSetupRequestValue::clear(name))
        .collect_vec();
        let existing_setup = vec![
            ("blockchain-service-url", "https://booga.com"),
            ("clandestine-port", "4321"),
            (
                "consuming-private-key",
                "7766554433221100776655443322110077665544332211007766554433221100",
            ),
            ("data-directory", "booga"),
            ("db-password", "drowssap"),
            ("dns-servers", "4.4.4.4"),
            (
                "earning-wallet",
                "0x9876543210987654321098765432109876543210",
            ),
            ("gas-price", "5"),
            ("ip", "1.2.3.4"),
            ("neighborhood-mode", "consume-only"),
            (
                "neighbors",
                "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@9.10.11.12:9101",
            ),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "6666:6666:agoob"),
        ]
        .into_iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                UiSetupResponseValue::new(name, value, Set),
            )
        })
        .collect::<SetupCluster>();
        let subject = SetupReporterReal::new();

        let result = subject.get_modified_setup(existing_setup, params).unwrap();

        let expected_result = vec![
            ("blockchain-service-url", "https://example.com", Configured),
            ("chain", "ropsten", Configured),
            ("clandestine-port", "1234", Configured),
            ("config-file", "config.toml", Default),
            ("consuming-private-key", "0011223344556677001122334455667700112233445566770011223344556677", Configured),
            ("data-directory", home_dir.to_str().unwrap(), Configured),
            ("db-password", "password", Configured),
            ("dns-servers", "8.8.8.8", Configured),
            (
                "earning-wallet",
                "0x0123456789012345678901234567890123456789",
                Configured,
            ),
            ("gas-price", "50", Configured),
            ("ip", "4.3.2.1", Configured),
            ("log-level", "error", Configured),
            ("neighborhood-mode", "originate-only", Configured),
            ("neighbors", "MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@1.2.3.4:1234,MTIzNDU2Nzg5MTEyMzQ1Njc4OTIxMjM0NTY3ODkzMTI@5.6.7.8:5678", Configured),
            #[cfg(not(target_os = "windows"))]
            ("real-user", "9999:9999:booga", Configured),
        ]
        .into_iter()
        .map(|(name, value, status)| {
            (
                name.to_string(),
                UiSetupResponseValue::new(name, value, status),
            )
        })
        .collect_vec();
        let presentable_result = result
            .into_iter()
            .sorted_by_key(|(k, _)| k.clone())
            .collect_vec();
        assert_eq!(presentable_result, expected_result);
    }

    #[test]
    fn calculate_fundamentals_with_only_environment() {
        let _guard = EnvironmentGuard::new();
        vec![
            ("SUB_CHAIN", "ropsten"),
            ("SUB_DATA_DIRECTORY", "env_dir"),
            ("SUB_REAL_USER", "9999:9999:booga"),
        ]
        .into_iter()
        .for_each(|(name, value)| std::env::set_var(name, value));
        let setup = vec![]
            .into_iter()
            .map(|(k, v, s): (&str, &str, UiSetupResponseValueStatus)| {
                (k.to_string(), UiSetupResponseValue::new(k, v, s))
            })
            .collect::<SetupCluster>();

        let (real_user, data_directory_opt, chain_name) =
            SetupReporterReal::calculate_fundamentals(setup).unwrap();

        assert_eq!(
            real_user,
            crate::bootstrapper::RealUser::new(
                Some(9999),
                Some(9999),
                Some(PathBuf::from("booga"))
            )
        );
        assert_eq!(data_directory_opt, Some(PathBuf::from("env_dir")));
        assert_eq!(chain_name, "ropsten".to_string());
    }

    #[test]
    fn calculate_fundamentals_with_environment_and_obsolete_setup() {
        let _guard = EnvironmentGuard::new();
        vec![
            ("SUB_CHAIN", "ropsten"),
            ("SUB_DATA_DIRECTORY", "env_dir"),
            ("SUB_REAL_USER", "9999:9999:booga"),
        ]
        .into_iter()
        .for_each(|(name, value)| std::env::set_var(name, value));
        let setup = vec![
            ("chain", "dev", Configured),
            ("data-directory", "setup_dir", Default),
            ("real-user", "1111:1111:agoob", Configured),
        ]
        .into_iter()
        .map(|(k, v, s)| (k.to_string(), UiSetupResponseValue::new(k, v, s)))
        .collect::<SetupCluster>();

        let (real_user, data_directory_opt, chain_name) =
            SetupReporterReal::calculate_fundamentals(setup).unwrap();

        assert_eq!(
            real_user,
            crate::bootstrapper::RealUser::new(
                Some(9999),
                Some(9999),
                Some(PathBuf::from("booga"))
            )
        );
        assert_eq!(data_directory_opt, Some(PathBuf::from("env_dir")));
        assert_eq!(chain_name, "ropsten".to_string());
    }

    #[test]
    fn calculate_fundamentals_with_environment_and_overriding_setup() {
        let _guard = EnvironmentGuard::new();
        vec![
            ("SUB_CHAIN", "ropsten"),
            ("SUB_DATA_DIRECTORY", "env_dir"),
            ("SUB_REAL_USER", "9999:9999:booga"),
        ]
        .into_iter()
        .for_each(|(name, value)| std::env::set_var(name, value));
        let setup = vec![
            ("chain", "dev", Set),
            ("data-directory", "setup_dir", Set),
            ("real-user", "1111:1111:agoob", Set),
        ]
        .into_iter()
        .map(|(k, v, s): (&str, &str, UiSetupResponseValueStatus)| {
            (k.to_string(), UiSetupResponseValue::new(k, v, s))
        })
        .collect::<SetupCluster>();

        let (real_user, data_directory_opt, chain_name) =
            SetupReporterReal::calculate_fundamentals(setup).unwrap();

        assert_eq!(
            real_user,
            crate::bootstrapper::RealUser::new(
                Some(1111),
                Some(1111),
                Some(PathBuf::from("agoob"))
            )
        );
        assert_eq!(data_directory_opt, Some(PathBuf::from("setup_dir")));
        assert_eq!(chain_name, "dev".to_string());
    }

    #[test]
    fn calculate_fundamentals_with_setup_and_no_environment() {
        let _guard = EnvironmentGuard::new();
        vec![]
            .into_iter()
            .for_each(|(name, value): (&str, &str)| std::env::set_var(name, value));
        let setup = vec![
            ("chain", "dev", Configured),
            ("data-directory", "setup_dir", Default),
            ("real-user", "1111:1111:agoob", Configured),
        ]
        .into_iter()
        .map(|(k, v, s)| (k.to_string(), UiSetupResponseValue::new(k, v, s)))
        .collect::<SetupCluster>();

        let (real_user, data_directory_opt, chain_name) =
            SetupReporterReal::calculate_fundamentals(setup).unwrap();

        assert_eq!(
            real_user,
            crate::bootstrapper::RealUser::new(
                Some(1111),
                Some(1111),
                Some(PathBuf::from("agoob"))
            )
        );
        assert_eq!(data_directory_opt, Some(PathBuf::from("setup_dir")));
        assert_eq!(chain_name, "dev".to_string());
    }

    #[test]
    fn calculate_fundamentals_with_neither_setup_nor_environment() {
        let _guard = EnvironmentGuard::new();
        vec![]
            .into_iter()
            .for_each(|(name, value): (&str, &str)| std::env::set_var(name, value));
        let setup = vec![]
            .into_iter()
            .map(|(k, v, s): (&str, &str, UiSetupResponseValueStatus)| {
                (k.to_string(), UiSetupResponseValue::new(k, v, s))
            })
            .collect::<SetupCluster>();

        let (real_user, data_directory_opt, chain_name) =
            SetupReporterReal::calculate_fundamentals(setup).unwrap();

        assert_eq!(real_user, crate::bootstrapper::RealUser::null().populate());
        assert_eq!(data_directory_opt, None);
        assert_eq!(chain_name, "mainnet".to_string());
    }

    #[test]
    fn blanking_a_parameter_with_a_default_produces_that_default() {
        let _guard = EnvironmentGuard::new();
        let subject = SetupReporterReal::new();

        let result = subject
            .get_modified_setup(
                HashMap::new(),
                vec![
                    UiSetupRequestValue::new("ip", "1.2.3.4"),
                    UiSetupRequestValue::clear("chain"),
                ],
            )
            .unwrap();

        let actual_chain = result.get("chain").unwrap();
        assert_eq!(
            actual_chain,
            &UiSetupResponseValue::new("chain", DEFAULT_CHAIN_NAME, Default)
        );
    }

    #[test]
    fn choose_uisrv_chooses_higher_priority_incoming_over_lower_priority_existing() {
        let existing = UiSetupResponseValue::new ("name", "existing", Configured);
        let incoming = UiSetupResponseValue::new ("name", "incoming", Set);

        let result = SetupReporterReal::choose_uisrv(&existing, &incoming);

        assert_eq! (result, &incoming);
    }

    #[test]
    fn choose_uisrv_chooses_higher_priority_existing_over_lower_priority_incoming() {
        let existing = UiSetupResponseValue::new ("name", "existing", Set);
        let incoming = UiSetupResponseValue::new ("name", "incoming", Configured);

        let result = SetupReporterReal::choose_uisrv(&existing, &incoming);

        assert_eq! (result, &existing);
    }

    #[test]
    fn choose_uisrv_chooses_incoming_over_existing_for_equal_priority() {
        let existing = UiSetupResponseValue::new ("name", "existing", Set);
        let incoming = UiSetupResponseValue::new ("name", "incoming", Set);

        let result = SetupReporterReal::choose_uisrv(&existing, &incoming);

        assert_eq! (result, &incoming);
    }

    #[test]
    fn chain_computed_default() {
        let subject = Chain {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, Some((DEFAULT_CHAIN_NAME.to_string(), Default)));
    }

    #[test]
    fn clandestine_port_computed_default_present() {
        let persistent_config = PersistentConfigurationMock::new().clandestine_port_result(1234);
        let subject = ClandestinePort {};

        let result = subject.computed_default(
            &BootstrapperConfig::new(),
            &Some(Box::new(persistent_config)),
            &None,
        );

        assert_eq!(result, Some(("1234".to_string(), Configured)))
    }

    #[test]
    fn clandestine_port_computed_default_absent() {
        let subject = ClandestinePort {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, None)
    }

    #[test]
    fn data_directory_computed_default() {
        let real_user = RealUser::null().populate();
        let expected = data_directory_from_context(&real_user, &None, DEFAULT_CHAIN_NAME)
            .to_string_lossy()
            .to_string();
        let mut config = BootstrapperConfig::new();
        config.real_user = real_user;
        config.blockchain_bridge_config.chain_id = chain_id_from_name(DEFAULT_CHAIN_NAME);

        let subject = DataDirectory {};

        let result = subject.computed_default(&config, &None, &None);

        assert_eq!(result, Some((expected, Default)))
    }

    #[test]
    fn dns_servers_computed_default() {
        let subject = DnsServers {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, Some(("1.1.1.1".to_string(), Default)))
    }

    #[test]
    fn earning_wallet_computed_default_configured() {
        let mut config = BootstrapperConfig::new();
        config.earning_wallet = Wallet::new("0x1234567890123456789012345678901234567890");
        let subject = EarningWallet {};

        let result = subject.computed_default(&config, &None, &None);

        assert_eq!(
            result,
            Some((
                "0x1234567890123456789012345678901234567890".to_string(),
                Configured
            ))
        )
    }

    #[test]
    fn earning_wallet_computed_default_default() {
        let mut config = BootstrapperConfig::new();
        config.earning_wallet = DEFAULT_EARNING_WALLET.clone();
        let subject = EarningWallet {};

        let result = subject.computed_default(&config, &None, &None);

        assert_eq!(result, Some((DEFAULT_EARNING_WALLET.to_string(), Default)))
    }

    #[test]
    fn gas_price_computed_default_present() {
        let persistent_config = PersistentConfigurationMock::new().gas_price_result(57);
        let subject = GasPrice {};

        let result = subject.computed_default(
            &BootstrapperConfig::new(),
            &Some(Box::new(persistent_config)),
            &None,
        );

        assert_eq!(result, Some(("57".to_string(), Configured)))
    }

    #[test]
    fn gas_price_computed_default_absent() {
        let subject = GasPrice {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, None)
    }

    #[test]
    fn log_level_computed_default() {
        let subject = LogLevel {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, Some(("warn".to_string(), Default)))
    }

    #[test]
    fn neighborhood_mode_computed_default() {
        let subject = NeighborhoodMode {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, Some(("standard".to_string(), Default)))
    }

    #[test]
    fn neighbors_computed_default_present_present_present_ok() {
        let past_neighbors_params_arc = Arc::new(Mutex::new(vec![]));
        let persistent_config = PersistentConfigurationMock::new()
            .past_neighbors_params(&past_neighbors_params_arc)
            .past_neighbors_result(Ok(Some(vec![
                NodeDescriptor::from_str(
                    main_cryptde(),
                    "MTEyMjMzNDQ1NTY2Nzc4ODExMjIzMzQ0NTU2Njc3ODg@1.2.3.4:1234",
                )
                .unwrap(),
                NodeDescriptor::from_str(
                    main_cryptde(),
                    "ODg3NzY2NTU0NDMzMjIxMTg4Nzc2NjU1NDQzMzIyMTE@4.3.2.1:4321",
                )
                .unwrap(),
            ])));
        let subject = Neighbors {};

        let result = subject.computed_default(
            &BootstrapperConfig::new(),
            &Some(Box::new(persistent_config)),
            &Some("password".to_string()),
        );

        assert_eq! (result, Some (("MTEyMjMzNDQ1NTY2Nzc4ODExMjIzMzQ0NTU2Njc3ODg@1.2.3.4:1234,ODg3NzY2NTU0NDMzMjIxMTg4Nzc2NjU1NDQzMzIyMTE@4.3.2.1:4321".to_string(), Configured)));
        let past_neighbors_params = past_neighbors_params_arc.lock().unwrap();
        assert_eq!(*past_neighbors_params, vec!["password".to_string()])
    }

    #[test]
    fn neighbors_computed_default_present_present_err() {
        let past_neighbors_params_arc = Arc::new(Mutex::new(vec![]));
        let persistent_config = PersistentConfigurationMock::new()
            .past_neighbors_params(&past_neighbors_params_arc)
            .past_neighbors_result(Err(PersistentConfigError::PasswordError));
        let subject = Neighbors {};

        let result = subject.computed_default(
            &BootstrapperConfig::new(),
            &Some(Box::new(persistent_config)),
            &Some("password".to_string()),
        );

        assert_eq!(result, None);
        let past_neighbors_params = past_neighbors_params_arc.lock().unwrap();
        assert_eq!(*past_neighbors_params, vec!["password".to_string()])
    }

    #[test]
    fn neighbors_computed_default_present_absent() {
        // absence of configured result will cause panic if past_neighbors is called
        let persistent_config = PersistentConfigurationMock::new();
        let subject = Neighbors {};

        let result = subject.computed_default(
            &BootstrapperConfig::new(),
            &Some(Box::new(persistent_config)),
            &None,
        );

        assert_eq!(result, None);
    }

    #[test]
    fn neighbors_computed_default_absent() {
        let subject = Neighbors {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn real_user_computed_default() {
        let subject = crate::daemon::setup_reporter::RealUser {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(
            result,
            Some((RealUser::default().populate().to_string(), Default))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn real_user_computed_default() {
        let subject = crate::daemon::setup_reporter::RealUser {};

        let result = subject.computed_default(&BootstrapperConfig::new(), &None, &None);

        assert_eq!(result, None);
    }

    fn verify_requirements(
        subject: &dyn ValueRetriever,
        param_name: &str,
        value_predictions: Vec<(&str, bool)>,
    ) {
        value_predictions
            .into_iter()
            .for_each(|(param_value, prediction)| {
                let params = vec![(
                    param_name.to_string(),
                    UiSetupResponseValue::new(param_name, param_value, Set),
                )]
                .into_iter()
                .collect::<SetupCluster>();

                let result = subject.is_required(&params);

                assert_eq!(result, prediction, "{:?}", params);
            })
    }

    fn verify_needed_for_blockchain(subject: &dyn ValueRetriever) {
        verify_requirements(
            subject,
            "neighborhood-mode",
            vec![
                ("standard", true),
                ("zero-hop", false),
                ("originate-only", true),
                ("consume-only", true),
            ],
        );
    }

    #[test]
    fn ip_requirements() {
        verify_requirements(
            &Ip {},
            "neighborhood-mode",
            vec![
                ("standard", true),
                ("zero-hop", false),
                ("originate-only", false),
                ("consume-only", false),
            ],
        );
    }

    #[test]
    fn neighbors_requirements() {
        verify_requirements(
            &Neighbors {},
            "neighborhood-mode",
            vec![
                ("standard", false),
                ("zero-hop", false),
                ("originate-only", true),
                ("consume-only", true),
            ],
        );
    }

    #[test]
    fn blockchain_requirements() {
        verify_needed_for_blockchain(&BlockchainServiceUrl {});
        verify_needed_for_blockchain(&DbPassword {});
        verify_needed_for_blockchain(&EarningWallet {});
        verify_needed_for_blockchain(&GasPrice {});
    }

    #[test]
    fn dumb_requirements() {
        let params = HashMap::new();
        assert_eq!(BlockchainServiceUrl {}.is_required(&params), true);
        assert_eq!(Chain {}.is_required(&params), true);
        assert_eq!(ClandestinePort {}.is_required(&params), true);
        assert_eq!(ConfigFile {}.is_required(&params), false);
        assert_eq!(ConsumingPrivateKey {}.is_required(&params), false);
        assert_eq!(DataDirectory {}.is_required(&params), true);
        assert_eq!(DbPassword {}.is_required(&params), true);
        assert_eq!(DnsServers {}.is_required(&params), true);
        assert_eq!(EarningWallet {}.is_required(&params), true);
        assert_eq!(GasPrice {}.is_required(&params), true);
        assert_eq!(Ip {}.is_required(&params), true);
        assert_eq!(LogLevel {}.is_required(&params), true);
        assert_eq!(NeighborhoodMode {}.is_required(&params), true);
        assert_eq!(Neighbors {}.is_required(&params), true);
        assert_eq!(
            crate::daemon::setup_reporter::RealUser {}.is_required(&params),
            false
        );
    }

    #[test]
    fn run_me_privileged() {
        let real_user = RealUser::null().populate();
        let directory = data_directory_from_context(&real_user, &None, "mainnet");
        eprintln!("default data directory: {:?}", directory);
    }
}
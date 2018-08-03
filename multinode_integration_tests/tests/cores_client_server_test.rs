// Copyright (c) 2017-2018, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
extern crate multinode_integration_tests_lib;
extern crate node_lib;
extern crate regex;
extern crate serde_cbor;
extern crate sub_lib;
extern crate hopper_lib;
extern crate base64;

use multinode_integration_tests_lib::substratum_cores_client::SubstratumCoresClient;
use multinode_integration_tests_lib::substratum_cores_server::SubstratumCoresServer;
use multinode_integration_tests_lib::substratum_node_cluster::SubstratumNodeCluster;
use node_lib::discriminator::DiscriminatorFactory;
use node_lib::json_masquerader::JsonMasquerader;
use sub_lib::cryptde::CryptDE;
use sub_lib::cryptde_null::CryptDENull;
use sub_lib::dispatcher::Component;
use sub_lib::hopper::ExpiredCoresPackage;
use sub_lib::hopper::IncipientCoresPackage;
use sub_lib::route::Route;
use sub_lib::route::RouteSegment;
use std::net::SocketAddr;
use multinode_integration_tests_lib::substratum_node::NodeStartupConfig;
use node_lib::json_discriminator_factory::JsonDiscriminatorFactory;
use multinode_integration_tests_lib::substratum_node::NeighborConfig;
use std::time::Duration;

#[test]
fn relay_cores_package () {
    let mut cluster = SubstratumNodeCluster::new (vec! (NodeStartupConfig::new (vec! (4663), vec!())));

    let cryptde = CryptDENull::new ();
    let factories: Vec<Box<DiscriminatorFactory>> = vec! (Box::new (JsonDiscriminatorFactory::new ()));
    let masquerader = JsonMasquerader::new ();
    let mut server = SubstratumCoresServer::new (4663, factories, &cryptde);
    let mut client = SubstratumCoresClient::new (server.local_addr (), &cryptde);
    let mut route = Route::new (
        vec! (
            RouteSegment::new (vec! (&cryptde.public_key(), &cryptde.public_key()), Component::Neighborhood)
        ),
        &cryptde
    ).unwrap ();
    let payload = String::from ("Booga booga!");
    let incipient = IncipientCoresPackage::new (route.clone (), payload, &cryptde.public_key());

    client.transmit_package(incipient, &masquerader, cryptde.public_key());
    let expired: ExpiredCoresPackage = server.wait_for_package (Duration::from_millis(1000));

    cluster.stop_all ();
    route.shift (&cryptde.private_key (), &cryptde);
    assert_eq! (expired.remaining_route, route);
    assert_eq! (serde_cbor::de::from_slice::<String> (&expired.payload.data[..]).unwrap (), String::from ("Booga booga!"));
}

#[ignore] // TODO FIXME re-enable when new Rust integration test harness is finished (SC-377)
#[test]
fn send_and_receive_masqueraded_cores_package_through_node() {
    let payload = "serious web request aaaaaaaahhhhhhhhhhhhhhhhhhhhhh";
    let json_masquerader = JsonMasquerader::new();
    let json_discrimination_port: u16 = 4554;
    let mut test_cryptde = CryptDENull::new();
    test_cryptde.generate_key_pair();
    let factories: Vec<Box<DiscriminatorFactory>> = vec!(Box::new(JsonDiscriminatorFactory::new()));

    let mut test_server = SubstratumCoresServer::new(json_discrimination_port, factories, &test_cryptde);

    let server_neighbor_arg = NeighborConfig::new(
        test_cryptde.public_key(),
        test_server.local_addr().ip(),
        vec!(test_server.local_addr().port()),
    );
    println!("test addr: {:?}", test_server.local_addr());

    let mut cluster = SubstratumNodeCluster::new(vec!(NodeStartupConfig::new(vec!(json_discrimination_port), vec!(server_neighbor_arg))));
    let node1_ip_addr = cluster.get_node("test_node_1").unwrap().get_ip_address();
    let node1_socket_addr = SocketAddr::new(node1_ip_addr, json_discrimination_port);
    let node1_public_key = cluster.get_node("test_node_1").unwrap().get_public_key();

    let mut client = SubstratumCoresClient::new(node1_socket_addr, &test_cryptde);

    // Create an incipient cores package
    let route_segments = RouteSegment::new(vec!(&test_cryptde.public_key(), &node1_public_key, &test_cryptde.public_key()), Component::ProxyClient);
    let route = Route::new(
        vec!(route_segments),
        &test_cryptde,
    ).unwrap();
    println!("test public key: {:?}", &test_cryptde.public_key());
    println!("node public key: {:?}", &node1_public_key);

    let incipient_cores_package = IncipientCoresPackage::new(route, payload, &test_cryptde.public_key());

    // Send masqueraded live cores package to the node
    client.transmit_package(incipient_cores_package, &json_masquerader, node1_public_key);

    cluster.stop_all ();
    // It should get routed and remasked and sent back
    let result_cores_package = test_server.wait_for_package(Duration::from_millis(8000));
    assert_eq!(serde_cbor::de::from_slice::<String>(&result_cores_package.payload.data[..]).unwrap(), payload);
}

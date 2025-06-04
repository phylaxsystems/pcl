use alloy::node_bindings::Anvil;
use alloy::{
    hex,
    signers::k256::ecdsa::SigningKey,
};
use assertion_da_client::DaClient;
use chrono::DateTime;
use int_test_utils::{
    deploy_dapp,
    deploy_test_da,
};
use pcl_common::args::CliArgs;
use pcl_core::config::CliConfig;
use pcl_core::{
    assertion_da::DaStoreArgs,
    assertion_submission::DappSubmitArgs,
    config::UserAuth,
    error::{
        AuthError,
        DaSubmitError,
        DappSubmitError,
    },
    project::{
        ProjectCommand,
        ProjectSubcommands,
    },
};
use pcl_phoundry::phorge::BuildAndFlattenArgs;
use std::{
    collections::HashMap,
    path::PathBuf,
};

#[derive(Debug, Default)]
pub struct TestSetup {
    pub root: Option<PathBuf>,
    pub assertion_contract: Option<String>,
    pub constructor_args: Vec<String>,
    pub project: Option<String>,
    pub json: bool,
}

impl TestSetup {
    pub fn new() -> Self {
        Self {
            root: None,
            assertion_contract: None,
            json: false,
            constructor_args: vec![],
            project: None,
        }
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.root = Some(root);
    }

    pub fn set_assertion_contract(&mut self, assertion_contract: String) {
        self.assertion_contract = Some(assertion_contract);
    }

    pub fn set_constructor_args(&mut self, constructor_args: Vec<String>) {
        self.constructor_args = constructor_args;
    }

    pub fn set_project(&mut self, project: String) {
        self.project = Some(project);
    }

    pub async fn build(&self) -> Result<TestRunner, DaSubmitError> {
        let anvil = Anvil::new().spawn();
        let rpc_url = anvil.endpoint();
        let (_handle, da_url) = deploy_test_da(SigningKey::random(&mut rand::thread_rng())).await;
        let (dapp_port, _dapp_handle) = deploy_dapp(
            &PathBuf::from("../../lib/credible-layer-dapp/apps/dapp/"),
            &rpc_url,
            &da_url.to_string(),
        )
        .unwrap();

        let build_and_flatten_args = BuildAndFlattenArgs {
            root: Some(
                self.root
                    .clone()
                    .unwrap_or(PathBuf::from("../../testdata/mock-protocol")),
            ),
            assertion_contract: self
                .assertion_contract
                .clone()
                .unwrap_or("NoArgsAssertion".to_string()),
        };

        let dapp_submit_args = DappSubmitArgs {
            dapp_url: format!("http://localhost:{dapp_port}/api/v1"),
            project_name: Some(self.project.clone().unwrap_or("test-project".to_string())),
            assertion_keys: None,
        };
        println!("dapp_submit_args: {:?}", dapp_submit_args.dapp_url);

        let da_store_args = DaStoreArgs {
            url: format!("http://{da_url}"),
            args: build_and_flatten_args,
            constructor_args: self.constructor_args.clone(),
        };

        let cli_config = CliConfig {
            auth: None,
            assertions_for_submission: HashMap::new(),
        };

        let cli_args: CliArgs = CliArgs {
            json: self.json,
            config_dir: None,
        };

        let test_runner = TestRunner {
            cli_args,
            cli_config,
            da_store_args,
            project_name: self.project.clone().unwrap_or("test-project".to_string()),
            dapp_submit_args,
            da_client: DaClient::new(&format!("http://{da_url}")).unwrap(),
            _da_handle: _handle,
        };
        Ok(test_runner)
    }
}

pub struct TestRunner {
    pub cli_args: CliArgs,
    pub da_store_args: DaStoreArgs,
    pub dapp_submit_args: DappSubmitArgs,
    pub cli_config: CliConfig,
    pub da_client: DaClient,
    pub project_name: String,
    pub _da_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl TestRunner {
    pub async fn store_assertion(&mut self) -> Result<(), DaSubmitError> {
        self.da_store_args
            .run(&self.cli_args, &mut self.cli_config)
            .await?;
        Ok(())
    }

    pub fn auth(&mut self) -> Result<(), AuthError> {
        self.cli_config.auth = Some(UserAuth {
            access_token: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIweGYzOWZkNmU1MWFhZDg4ZjZmNGNlNmFiODgyNzI3OWNmZmZiOTIyNjYiLCJqdGkiOiI3YmI2YzA5ZS05NDZlLTRhOGYtOGVkOS0zYmIwNzA3NTIyYzUiLCJzaWQiOiI4YjkzZDk3NS0zYTYxLTQ1MWItOGE3NS02YjBjYzZjOThhZTAiLCJzY29wZSI6ImNsaSIsImlhdCI6MTc0ODk2MjY4NSwiZXhwIjoxNzQ5NTY3NDg1fQ.OqjX9EDmCwRKSYygdjXjaUcbCwrjqN2de_STNL21Nkg".to_string(),
            refresh_token: "c5529b2208beb3e7ccf5e189d4cf19a32e91b4bb1b6da499e100b7786016b895443bd19f89ea837e".to_string(),
            user_address: "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266".parse().unwrap(),
            expires_at: DateTime::from_timestamp(1748962932, 0).unwrap(),
        });

        Ok(())
    }

    pub async fn create_project(&mut self) -> Result<(), DappSubmitError> {
        // Create project
        let create_project_args = ProjectCommand {
            command: ProjectSubcommands::Create {
                project_name: self.project_name.clone(),
                project_description: None,
                profile_image_url: None,
                assertion_adopters: vec![],
                chain_id: 1,
            },
            base_url: self.dapp_submit_args.dapp_url.clone(),
        };
        create_project_args.run(&mut self.cli_config).await?;
        Ok(())
    }
    pub async fn submit_assertion(&mut self) -> Result<(), DappSubmitError> {
        self.dapp_submit_args
            .run(&self.cli_args, &mut self.cli_config)
            .await?;
        Ok(())
    }
    pub async fn assert_assertion_as_expected(&self, assertion_id: String) {
        let assertion_for_submission = self
            .cli_config
            .assertions_for_submission
            .get(&assertion_id.clone().into())
            .unwrap();

        let assertion = self
            .da_client
            .fetch_assertion(
                assertion_for_submission
                    .assertion_id
                    .clone()
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            assertion.prover_signature,
            hex::decode(assertion_for_submission.signature.clone()).unwrap()
        );
        assert_eq!(
            self.da_store_args.constructor_args,
            assertion_for_submission.constructor_args
        );
    }
}

#[tokio::test]
pub async fn test_run_submit_example() {
    let test_setup = TestSetup::new();
    let mut test_runner = test_setup.build().await.unwrap();

    println!("Authorizing...");
    // Auth
    test_runner.auth().unwrap();

    println!("Creating project...");
    // Create project
    test_runner.create_project().await.unwrap();

    println!("Storing assertion...");
    // Store assertion
    test_runner.store_assertion().await.unwrap();

    println!("Asserting assertion as expected...");
    test_runner
        .assert_assertion_as_expected("1".to_string())
        .await;

    println!("Submitting assertion...");
    test_runner.submit_assertion().await.unwrap();
}

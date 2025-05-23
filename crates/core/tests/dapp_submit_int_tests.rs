mod common;

#[cfg(test)]
mod dapp_submit_int_tests {
    use crate::common::dapp_submit_harness::TestSetup;
    use pcl_core::error::DappSubmitError;

    // Test dapp submission with project and assertions, validate the da interactions with the dapp work as expected.
    #[tokio::test]
    async fn test_dapp_submit_with_project_and_assertions() {
        let test_setup = TestSetup::new();
        let mut test_runner = test_setup.build().await.unwrap();
        test_runner.auth().unwrap();
        test_runner.create_project().await.unwrap();
        test_runner.store_assertion().await.unwrap();
        test_runner.submit_assertion().await.unwrap();
    }

    #[tokio::test]
    async fn test_dapp_submit_no_projects() {
        let test_setup = TestSetup::new();
        let mut test_runner = test_setup.build().await.unwrap();
        test_runner.auth().unwrap();
        let result = test_runner.submit_assertion().await;
        assert!(matches!(
            result.unwrap_err(),
            DappSubmitError::NoProjectsFound
        ));
    }
    #[tokio::test]
    async fn test_dapp_submit_no_auth() {
        let test_setup = TestSetup::new();
        let mut test_runner = test_setup.build().await.unwrap();
        test_runner.auth().unwrap();
        test_runner.create_project().await.unwrap();
        test_runner.store_assertion().await.unwrap();
        test_runner.cli_config.auth = None;
        let result = test_runner.submit_assertion().await;
        assert!(matches!(result.unwrap_err(), DappSubmitError::NoAuthToken));
    }
}

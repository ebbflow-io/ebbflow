mod mockebb;

#[cfg(test)]
mod basic_tests_v0 {
    use crate::mockebb::listen_and_process;
    use tokio::net::TcpStream;
    use std::time::Duration;

    const MOCKEBBSPAWNDELAY: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn passthrough_basic_bytes() {
        let testclientport = 49193;
        let customerport = 49194;

        tokio::spawn(listen_and_process(customerport, testclientport));
        tokio::time::delay_for(MOCKEBBSPAWNDELAY).await;
        println!("Spawned");
        let client = TcpStream::connect(format!("127.0.0.1:{}", customerport)).await.unwrap();
        println!("Connected");

        assert!(false);
    }
}
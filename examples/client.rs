use hyper::{Client, Uri};

async fn main() {
    loop {


        sleep
    }
}

async fn request() -> Result<(), reqwest::Error> {
    let client = Client::new();

    let res = reqwest::Client::new()
        .get("https://0.0.0.0:4443")
        .send()
        .await.;

    println!("Status: {}", res.status());

    let body = res.text().await?;

    println!("Body:\n\n{}", body);
}
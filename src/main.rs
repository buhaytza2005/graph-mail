use graph_rs_sdk::oauth::{AccessToken, OAuth};
use graph_rs_sdk::GraphResult;
use graph_rs_sdk::Graph;
use std::time::Duration;
use std::env;
use dotenv::dotenv;
use std::fs::File;
use std::io::{Read, Write};
use serde::{Serialize, Deserialize};


const TARGET_EMAIL: &str = "<EMAIL YOU WANT TO SEND TO>";



// Update the client id with your own.
fn get_oauth() -> OAuth {
    let client_id: String = env::var("CLIENT_ID").unwrap();
    let mut oauth = OAuth::new();
    let device_code_endpoint :String = format!(r#"https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode"#, env::var("TENANT_ID").unwrap());
    let token_endpoint: String = format!(r#"https://login.microsoftonline.com/{}/oauth2/v2.0/token"#, env::var("TENANT_ID").unwrap());
    oauth
        .client_id(&client_id)
        .authorize_url(&device_code_endpoint)
        .refresh_token_url(&token_endpoint)
        .access_token_url(&token_endpoint)
        .add_scope("Mail.ReadWrite")
        .add_scope("offline_access");

    oauth
}


// Create a struct to hold the token data
#[derive(Serialize, Deserialize, Debug, Clone)]
struct TokenCache {
        access_token: AccessToken,
        refresh_token: String,
        expires_at: u64,
}

// Load token cache from a file
fn load_token_cache() -> Option<TokenCache> {
    if let Ok(mut file) = File::open("token_cache.json") {
        let mut json_data = String::new();
        if file.read_to_string(&mut json_data).is_ok() {
            if let Ok(token_cache) = serde_json::from_str(&json_data) {
                return Some(token_cache);
            }
        }
    }
    None
}

// Save token cache to a file
fn save_token_cache(token_cache: &TokenCache) -> std::io::Result<()> {
    let json_data = serde_json::to_string_pretty(token_cache)?;
    let mut file = File::create("token_cache.json")?;
    file.write_all(json_data.as_bytes())?;
    Ok(())
}

// When polling to wait on the user to enter a device code you should check the errors
// so that you know what to do next.
//
// authorization_pending: The user hasn't finished authenticating, but hasn't canceled the flow. Repeat the request after at least interval seconds.
// authorization_declined: The end user denied the authorization request. Stop polling and revert to an unauthenticated state.
// bad_verification_code: The device_code sent to the /token endpoint wasn't recognized. Verify that the client is sending the correct device_code in the request.
// expired_token: Value of expires_in has been exceeded and authentication is no longer possible with device_code. Stop polling and revert to an unauthenticated state.
async fn poll_for_access_token(
    device_code: &str,
    interval: u64,
    message: &str,
) -> GraphResult<AccessToken> {
    let mut oauth = get_oauth();
    oauth.device_code(device_code);

    let mut request = oauth.build_async().device_code();
    let response = request.access_token().send().await?;

    println!("{response:#?}");

    let status = response.status();

    let body: serde_json::Value = response.json().await?;
    println!("{body:#?}");

    if !status.is_success() {
        loop {
            // Wait the amount of seconds that interval is.
            std::thread::sleep(Duration::from_secs(interval));

            let response = request.access_token().send().await?;

            let status = response.status();
            //println!("{response:#?}");

            let body: serde_json::Value = response.json().await?;
            //println!("{body:#?}");

            if status.is_success() {
                let cache: AccessToken = handle_cache(body).await?;
                return Ok(cache)

            } else {
                let option_error = body["error"].as_str();

                if let Some(error) = option_error {
                    match error {
                        "authorization_pending" => println!("Still waiting on user to sign in"),
                        "authorization_declined" => panic!("user declined to sign in"),
                        "bad_verification_code" => println!("User is lost\n{message:#?}"),
                        "expired_token" => panic!("token has expired - user did not sign in"),
                        _ => {
                            panic!("This isn't the error we expected: {error:#?}");
                        }
                    }
                } else {
                    // Body should have error or we should bail.
                    panic!("Crap hit the fan");
                }
            }
        }
    } else {
        let cache: AccessToken = handle_cache(body).await?;
        return Ok(cache)
    }
}



async fn get_token_for_query() -> Result<AccessToken, Box<dyn std::error::Error>>{

    // start by loading the cache
    let token_cache = load_token_cache(); 

    // if there is a token, check to see if it's expired
    if let Some(cache) = token_cache{
        let cache2 = cache.clone();
        let current_time = chrono::Utc::now().timestamp() as u64;
        if current_time < cache.expires_at {
            let access_token = cache.access_token;
            println!("using cached token");
            return Ok(access_token)
        } else {
            
            println!("Invalid token stored, requesting new one");

            let mut acc_token = AccessToken::default();
            acc_token.set_refresh_token(&cache2.refresh_token);
            let mut oauth = get_oauth();
            oauth.access_token(acc_token);
            
            let mut handler = oauth.build_async().device_code();
            let response = handler.refresh_token().send().await?;
            let body: serde_json::Value = response.json().await?;
            let cache: AccessToken = handle_cache(body).await?;
            return Ok(cache)
        } 

    } else {
        println!("Really wrong!!!");
        return Err(Box::from(graph_rs_sdk::GraphFailure::default()))
    }
}


#[tokio::main]
async fn main() -> GraphResult<()> {
    dotenv().ok();

    //get access_token somehow, either from the cache or refresh it 
    let access_token = get_token_for_query().await;
    if let Ok(token) = access_token {
        send_mail(token.bearer_token()).await?;
    } else {
        // if no valid token in cache, cache missing or refresh token expired manually log in -
        // this will cache the token
        let mut oauth = get_oauth();
        let mut handler = oauth.build_async().device_code();
        let response = handler.authorization().send().await?;
        let json: serde_json::Value = response.json().await?;
        let device_code = json["device_code"].as_str().unwrap();
        let interval = json["interval"].as_u64().unwrap();
        let message = json["message"].as_str().unwrap();
        println!("{:#?}", message);
        let _access_token_json = poll_for_access_token(device_code, interval, message).await?;
        let token: AccessToken = oauth.get_access_token().unwrap();
        send_mail(token.bearer_token()).await?;
        println!("Done?");
    }

    Ok(())
}

async fn handle_cache(body: serde_json::Value)-> GraphResult<AccessToken> {
    //get copy of body, will need the body for exires_at
    let access_token_json = body.clone();
    let access_token: AccessToken = serde_json::from_value(access_token_json)?;

    let acc2 = access_token.clone();
    
    let token_cache: TokenCache = TokenCache {
        access_token: acc2,
        refresh_token: access_token.refresh_token().unwrap(),
        expires_at: chrono::Utc::now().timestamp() as u64 + body["expires_in"].as_u64().unwrap_or(0)
    };
    save_token_cache(&token_cache)?;
    Ok(access_token)


}
async fn send_mail(access_token: &str) -> GraphResult<()> {
    let client = Graph::new(access_token);

    let response = client
        .me()
        .send_mail(&serde_json::json!({
                "message": {
                "subject": "Meet for lunch?",
                "body": {
                    "contentType": "Text",
                    "content": "The new cafeteria is open."
                },
                "toRecipients": [
                    {
                        "emailAddress": {
                        "address": TARGET_EMAIL 
                        }
                    }
                ],
                "ccRecipients": [
                    {
                        "emailAddress": {
                        "address": TARGET_EMAIL
                        }
                    }
                ]
            },
            "saveToSentItems": "true"
        }))
        .send()
        .await?;

    println!("{response:#?}");

    Ok(())
}

use std::collections::HashMap;
use rocket::serde::json::Json;
use aw_models::User;
use rocket::http::Status;
use rocket::State;
use serde::Deserialize;
use crate::endpoints::{HttpErrorJson, ServerState};


#[derive(Deserialize, Clone, Copy)]
pub struct LoginModel<'r> {
    username: &'r str,
    password: &'r str,
}


#[post("/login",data="<input>")]
pub fn login(
    state: &State<ServerState>,
    input:Json<LoginModel>
) -> Result<Json<User>, HttpErrorJson> {
    let username = input.username.to_string();
    let password = input.password.to_string();
    if(username.is_empty() || password.is_empty()){
        let err_msg = format!(
            "No user"
        );
        return Err(HttpErrorJson::new(Status::BadRequest, err_msg));
    }
    let datastore = endpoints_get_lock!(state.datastore);
    match datastore.get_user(input.username.to_string()) {
        Ok(user) => Ok(Json(user)),
        Err(err) => Err(err.into()),
    }
}
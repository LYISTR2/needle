//! Administrator sessions, metadata and single-use outbound-agent enrollment.
use crate::{server::{App, Config, open_db}, shared};
use anyhow::{Result, ensure};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{Json, extract::{State, Path}, http::{HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{io::{self, BufRead}, path::Path as FsPath};

type Api<T> = std::result::Result<T, (StatusCode, &'static str)>;
fn internal(_: impl std::fmt::Display) -> (StatusCode, &'static str) { (StatusCode::INTERNAL_SERVER_ERROR, "操作失败，请查看服务日志") }

pub fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS administrator (id INTEGER PRIMARY KEY CHECK(id=1), username TEXT NOT NULL, password_hash TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions (token_hash TEXT PRIMARY KEY, csrf TEXT NOT NULL, expires INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS node_meta (node_id TEXT PRIMARY KEY, group_name TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '', expires_at TEXT NOT NULL DEFAULT '', quota_gib REAL NOT NULL DEFAULT 0, sort_order INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS enrollments (token_hash TEXT PRIMARY KEY, node_id TEXT NOT NULL UNIQUE, interface TEXT NOT NULL, expires INTEGER NOT NULL);")?;
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    ensure!((12..=256).contains(&password.len()), "密码须为 12–256 字节");
    let salt = SaltString::encode_b64(&hex::decode(shared::token()?)?[..16])?;
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt)?.to_string())
}
fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}
pub fn set_admin(path: &FsPath, username: &str) -> Result<()> {
    ensure!(!username.is_empty() && username.len() <= 64 && !username.chars().any(char::is_control), "invalid username");
    let mut password = String::new();
    io::stdin().lock().read_line(&mut password)?;
    let hash = hash_password(password.trim_end_matches(['\r', '\n']))?;
    let cfg: Config = shared::read_json(path)?;
    let mut db = open_db(&cfg.database)?;
    let tx = db.transaction()?;
    tx.execute("INSERT INTO administrator VALUES (1,?1,?2) ON CONFLICT(id) DO UPDATE SET username=excluded.username,password_hash=excluded.password_hash", params![username, hash])?;
    tx.execute("DELETE FROM sessions", [])?;
    tx.commit()?;
    println!("Administrator saved; all previous sessions revoked.");
    Ok(())
}
fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("cookie")?.to_str().ok()?.split(';').filter_map(|s| s.trim().split_once('=')).find(|(k,_)| *k == "needle_session")?.1;
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then_some(value)
}
fn cookie(app: &App, value: &str, age: u64) -> String {
    format!("needle_session={value}; Path={}/; Max-Age={age}; Secure; HttpOnly; SameSite=Strict", app.path_prefix)
}
pub(crate) fn authorize(app: &App, headers: &HeaderMap, mutation: bool) -> Api<String> {
    if mutation && headers.get("origin").and_then(|v| v.to_str().ok()) != Some(app.origin.as_str()) {
        return Err((StatusCode::FORBIDDEN, "请求来源不匹配"));
    }
    let token = cookie_token(headers).ok_or((StatusCode::UNAUTHORIZED, "请登录管理员账号"))?;
    let db = app.db.lock().map_err(internal)?;
    let csrf: Option<String> = db.query_row("SELECT csrf FROM sessions WHERE token_hash=?1 AND expires>?2", params![shared::digest(token), shared::now()], |r| r.get(0)).optional().map_err(internal)?;
    let csrf = csrf.ok_or((StatusCode::UNAUTHORIZED, "登录已过期"))?;
    if mutation && headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(csrf.as_str()) {
        return Err((StatusCode::FORBIDDEN, "会话校验失败，请刷新后重试"));
    }
    Ok(csrf)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Login { username: String, password: String }
pub(crate) async fn login(State(app): State<App>, headers: HeaderMap, Json(input): Json<Login>) -> Api<Response> {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(app.origin.as_str()) { return Err((StatusCode::FORBIDDEN,"请求来源不匹配")); }
    if input.username.len()>64 || input.password.len()>256 { return Err((StatusCode::BAD_REQUEST,"账号或密码过长")); }
    {
        let mut gate = app.login_gate.lock().map_err(internal)?;
        let now = shared::now();
        if now < *gate { return Err((StatusCode::TOO_MANY_REQUESTS,"登录过于频繁，请稍后重试")); }
        *gate = now + 3;
    }
    let saved: Option<(String,String)> = app.db.lock().map_err(internal)?.query_row("SELECT username,password_hash FROM administrator WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(internal)?;
    let (username, hash) = saved.ok_or((StatusCode::SERVICE_UNAVAILABLE,"尚未设置管理员，请在服务器执行 needle-admin set-admin"))?;
    let old_hash = hash.clone();
    let valid = tokio::task::spawn_blocking(move || verify(&input.password,&hash)).await.map_err(internal)?;
    if !valid || username != input.username { return Err((StatusCode::UNAUTHORIZED,"账号或密码错误")); }
    let token = shared::token().map_err(internal)?;
    let csrf = shared::token().map_err(internal)?;
    let mut db = app.db.lock().map_err(internal)?;
    let tx = db.transaction().map_err(internal)?;
    let current: String = tx.query_row("SELECT password_hash FROM administrator WHERE id=1",[],|r|r.get(0)).map_err(internal)?;
    if current != old_hash { return Err((StatusCode::UNAUTHORIZED,"密码已更新，请重新登录")); }
    tx.execute("DELETE FROM sessions WHERE expires<=?1",[shared::now()]).map_err(internal)?;
    let count: i64 = tx.query_row("SELECT count(*) FROM sessions",[],|r|r.get(0)).map_err(internal)?;
    if count >= 16 { tx.execute("DELETE FROM sessions WHERE token_hash=(SELECT token_hash FROM sessions ORDER BY expires LIMIT 1)",[]).map_err(internal)?; }
    tx.execute("INSERT INTO sessions VALUES (?1,?2,?3)",params![shared::digest(&token),csrf,shared::now()+43200]).map_err(internal)?;
    tx.commit().map_err(internal)?;
    let mut response = Json(json!({"username":username,"csrf":csrf,"admin":true})).into_response();
    response.headers_mut().insert("set-cookie",HeaderValue::from_str(&cookie(&app,&token,43200)).map_err(internal)?);
    Ok(response)
}
pub(crate) async fn session(State(app): State<App>, headers: HeaderMap) -> Api<Json<Value>> {
    let csrf = authorize(&app,&headers,false)?;
    let username: String = app.db.lock().map_err(internal)?.query_row("SELECT username FROM administrator WHERE id=1",[],|r|r.get(0)).map_err(internal)?;
    Ok(Json(json!({"username":username,"csrf":csrf,"admin":true})))
}
pub(crate) async fn logout(State(app): State<App>, headers: HeaderMap) -> Api<Response> {
    authorize(&app,&headers,true)?;
    if let Some(token) = cookie_token(&headers) { app.db.lock().map_err(internal)?.execute("DELETE FROM sessions WHERE token_hash=?1",[shared::digest(token)]).map_err(internal)?; }
    let mut response=StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert("set-cookie",HeaderValue::from_str(&cookie(&app,"",0)).map_err(internal)?);
    Ok(response)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordChange { current_password: String, new_password: String }
pub(crate) async fn change_password(State(app): State<App>, headers: HeaderMap, Json(input): Json<PasswordChange>) -> Api<StatusCode> {
    authorize(&app,&headers,true)?;
    if input.current_password.len()>256 || !(12..=256).contains(&input.new_password.len()) { return Err((StatusCode::BAD_REQUEST,"新密码须为 12–256 字节")); }
    let old: String=app.db.lock().map_err(internal)?.query_row("SELECT password_hash FROM administrator WHERE id=1",[],|r|r.get(0)).map_err(internal)?;
    let compare=old.clone();
    let hash=tokio::task::spawn_blocking(move || { ensure!(verify(&input.current_password,&compare),"当前密码错误"); hash_password(&input.new_password) }).await.map_err(internal)?.map_err(|_|(StatusCode::BAD_REQUEST,"当前密码错误或新密码不符合要求"))?;
    let mut db=app.db.lock().map_err(internal)?;
    let tx=db.transaction().map_err(internal)?;
    if tx.execute("UPDATE administrator SET password_hash=?1 WHERE id=1 AND password_hash=?2",params![hash,old]).map_err(internal)?==0 {return Err((StatusCode::CONFLICT,"密码已被更新"));}
    tx.execute("DELETE FROM sessions",[]).map_err(internal)?;
    tx.commit().map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeInput {
    id: String, name: String,
    #[serde(default)] group_name: String,
    #[serde(default)] note: String,
    #[serde(default)] expires_at: String,
    #[serde(default)] quota_gib: f64,
    #[serde(default)] sort_order: i32,
    #[serde(default = "default_interface")] interface: String,
}
fn default_interface() -> String { "eth0".into() }
impl NodeInput {
    fn validate(&self) -> Api<()> {
        shared::valid_id(&self.id).map_err(|_|(StatusCode::BAD_REQUEST,"节点 ID 只能包含英文字母、数字、短横线和下划线"))?;
        crate::agent::valid_interface(&self.interface).map_err(|_|(StatusCode::BAD_REQUEST,"网卡名称无效"))?;
        if self.name.trim().is_empty() || self.name.len()>80 || self.group_name.len()>64 || self.note.len()>500 || [&self.name,&self.group_name,&self.note].iter().any(|v|v.chars().any(char::is_control)) || !self.quota_gib.is_finite() || !(0.0..=1e9).contains(&self.quota_gib) { return Err((StatusCode::BAD_REQUEST,"节点名称、分组、备注或流量配额无效")); }
        if !self.expires_at.is_empty() {
            let b=self.expires_at.as_bytes();
            if b.len()!=10 || b[4]!=b'-' || b[7]!=b'-' || !b.iter().enumerate().all(|(i,c)| i==4||i==7||c.is_ascii_digit()) { return Err((StatusCode::BAD_REQUEST,"到期日期须为 YYYY-MM-DD")); }
            let year=self.expires_at[..4].parse::<u32>().unwrap_or(0); let month=self.expires_at[5..7].parse::<u32>().unwrap_or(0); let day=self.expires_at[8..].parse::<u32>().unwrap_or(0);
            let leap=year.is_multiple_of(4) && (!year.is_multiple_of(100)||year.is_multiple_of(400));
            let days=match month {4|6|9|11=>30,2=>if leap {29}else{28},1|3|5|7|8|10|12=>31,_=>0};
            if year<1970 || day==0 || day>days { return Err((StatusCode::BAD_REQUEST,"到期日期无效")); }
        }
        Ok(())
    }
    fn save(&self, db: &Connection) -> Api<()> {
        db.execute("INSERT INTO node_meta VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(node_id) DO UPDATE SET group_name=excluded.group_name,note=excluded.note,expires_at=excluded.expires_at,quota_gib=excluded.quota_gib,sort_order=excluded.sort_order",params![self.id,self.group_name,self.note,self.expires_at,self.quota_gib,self.sort_order]).map_err(internal)?;
        Ok(())
    }
}
fn issue(app: &App, db: &Connection, id: &str, interface: &str) -> Api<Value> {
    let token=shared::token().map_err(internal)?;
    db.execute("DELETE FROM enrollments WHERE expires<=?1 OR node_id=?2",params![shared::now(),id]).map_err(internal)?;
    db.execute("INSERT INTO enrollments VALUES (?1,?2,?3,?4)",params![shared::digest(&token),id,interface,shared::now()+900]).map_err(internal)?;
    let url=format!("{}{}",app.origin,app.path_prefix);
    // Token is supplied over stdin, not a URL, so reverse-proxy access logs never see it.
    let quoted_url = format!("'{}'", url.replace('\'', "'\"'\"'"));
    let command=format!("curl -fsSL {quoted_url}/install.sh -o needle-install.sh && sudo -v && printf '%s' '{token}' | sudo bash needle-install.sh agent --url {quoted_url} --enroll-stdin");
    Ok(json!({"command":command,"expires_in":900,"notice":"命令仅可使用一次，15 分钟后过期。重新安装会轮换节点凭据；旧 agent 将无法上报。"}))
}
pub(crate) async fn create_node(State(app): State<App>,headers: HeaderMap,Json(input): Json<NodeInput>) -> Api<Json<Value>> {
    authorize(&app,&headers,true)?; input.validate()?;
    let mut db=app.db.lock().map_err(internal)?; let tx=db.transaction().map_err(internal)?;
    let count:i64=tx.query_row("SELECT count(*) FROM nodes",[],|r|r.get(0)).map_err(internal)?;
    if count>=256 { return Err((StatusCode::BAD_REQUEST,"最多注册 256 个节点")); }
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=?1)",[&input.id],|r|r.get(0)).map_err(internal)?;
    if exists { return Err((StatusCode::CONFLICT,"节点 ID 已存在")); }
    tx.execute("INSERT INTO nodes(id,name,token_hash) VALUES (?1,?2,?3)",params![input.id,input.name,shared::digest(&shared::token().map_err(internal)?)]).map_err(internal)?;
    input.save(&tx)?; let output=issue(&app,&tx,&input.id,&input.interface)?; tx.commit().map_err(internal)?;
    Ok(Json(output))
}
pub(crate) async fn edit_node(State(app): State<App>,Path(id):Path<String>,headers:HeaderMap,Json(input):Json<NodeInput>)->Api<StatusCode>{
    authorize(&app,&headers,true)?;input.validate()?;
    if id!=input.id {return Err((StatusCode::BAD_REQUEST,"节点 ID 不可修改"));}
    let mut db=app.db.lock().map_err(internal)?; let tx=db.transaction().map_err(internal)?;
    if tx.execute("UPDATE nodes SET name=?1 WHERE id=?2",params![input.name,id]).map_err(internal)?==0 {return Err((StatusCode::NOT_FOUND,"节点不存在"));}
    input.save(&tx)?;tx.commit().map_err(internal)?;Ok(StatusCode::NO_CONTENT)
}
pub(crate) async fn delete_node(State(app):State<App>,Path(id):Path<String>,headers:HeaderMap)->Api<StatusCode>{
    authorize(&app,&headers,true)?;
    let mut db=app.db.lock().map_err(internal)?;let tx=db.transaction().map_err(internal)?;
    if tx.execute("DELETE FROM nodes WHERE id=?1",[&id]).map_err(internal)?==0{return Err((StatusCode::NOT_FOUND,"节点不存在"));}
    tx.execute("DELETE FROM node_meta WHERE node_id=?1",[&id]).map_err(internal)?;
    tx.execute("DELETE FROM enrollments WHERE node_id=?1",[&id]).map_err(internal)?;
    tx.commit().map_err(internal)?;Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Enrollment { interface:String }
pub(crate) async fn enroll_node(State(app):State<App>,Path(id):Path<String>,headers:HeaderMap,Json(input):Json<Enrollment>)->Api<Json<Value>>{
    authorize(&app,&headers,true)?;
    crate::agent::valid_interface(&input.interface).map_err(|_|(StatusCode::BAD_REQUEST,"网卡名称无效"))?;
    let mut db=app.db.lock().map_err(internal)?;let tx=db.transaction().map_err(internal)?;
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=?1)",[&id],|r|r.get(0)).map_err(internal)?;
    if !exists {return Err((StatusCode::NOT_FOUND,"节点不存在"));}
    let output=issue(&app,&tx,&id,&input.interface)?;tx.commit().map_err(internal)?;Ok(Json(output))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Redeem { token:String }
pub(crate) async fn redeem(State(app):State<App>,Json(input):Json<Redeem>)->Api<Json<Value>>{
    if input.token.len()!=64 || !input.token.bytes().all(|b|b.is_ascii_hexdigit()){return Err((StatusCode::UNAUTHORIZED,"安装凭据无效"));}
    let mut db=app.db.lock().map_err(internal)?;let tx=db.transaction().map_err(internal)?;
    let row:Option<(String,String)>=tx.query_row("SELECT node_id,interface FROM enrollments WHERE token_hash=?1 AND expires>?2",params![shared::digest(&input.token),shared::now()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(internal)?;
    let (id,interface)=row.ok_or((StatusCode::UNAUTHORIZED,"安装凭据已过期或已使用，请重新生成"))?;
    let token=shared::token().map_err(internal)?;
    if tx.execute("UPDATE nodes SET token_hash=?1 WHERE id=?2",params![shared::digest(&token),id]).map_err(internal)?==0{return Err((StatusCode::NOT_FOUND,"节点不存在"));}
    tx.execute("DELETE FROM enrollments WHERE node_id=?1",[&id]).map_err(internal)?;
    // Keep persisted sequence so a new machine starts above all previously accepted reports.
    let sequence:u64=tx.query_row("SELECT sequence FROM nodes WHERE id=?1",[&id],|r|r.get(0)).map_err(internal)?;
    tx.commit().map_err(internal)?;
    let cfg=crate::agent::Config{endpoint:app.origin.clone(),path_prefix:app.path_prefix.clone(),node_id:id,sequence_floor:sequence,token,state_dir:"/var/lib/needle-agent".into(),interfaces:vec![interface],interval_seconds:10,targets:vec![],ca_certificate:None};
    Ok(Json(json!({"config":cfg,"sequence_floor":sequence})))
}

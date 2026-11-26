//! 7TV, BetterTTV, and FrankerFaceZ emote sets. A set that cannot be fetched
//! counts as empty, since chat works without it.

use chat_core::{Emote, EmoteProvider};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const SEVEN_TV: &str = "https://7tv.io/v3";
const BTTV: &str = "https://api.betterttv.net/3/cached";
const FFZ: &str = "https://api.frankerfacez.com/v1";

pub async fn fetch_global_emotes(http: &reqwest::Client) -> Vec<Emote> {
    let (ffz, bttv, seven) = futures::join!(
        get::<FfzSets>(http, format!("{FFZ}/set/global")),
        get::<Vec<BttvEmote>>(http, format!("{BTTV}/emotes/global")),
        get::<SevenTvSet>(http, format!("{SEVEN_TV}/emote-sets/global")),
    );
    merge(ffz, bttv, seven)
}

pub(crate) async fn fetch_twitch_channel(http: &reqwest::Client, room_id: &str) -> Vec<Emote> {
    let (ffz, bttv, seven) = futures::join!(
        get::<FfzSets>(http, format!("{FFZ}/room/id/{room_id}")),
        get::<BttvUser>(http, format!("{BTTV}/users/twitch/{room_id}")),
        get::<SevenTvUser>(http, format!("{SEVEN_TV}/users/twitch/{room_id}")),
    );
    merge(
        ffz,
        bttv.map(BttvUser::emotes),
        seven.and_then(|user| user.emote_set),
    )
}

pub(crate) async fn fetch_kick_channel(http: &reqwest::Client, user_id: u64) -> Vec<Emote> {
    let seven = get::<SevenTvUser>(http, format!("{SEVEN_TV}/users/kick/{user_id}")).await;
    merge(None, None, seven.and_then(|user| user.emote_set))
}

pub(crate) async fn fetch_youtube_channel(http: &reqwest::Client, channel_id: &str) -> Vec<Emote> {
    let (bttv, seven) = futures::join!(
        get::<BttvUser>(http, format!("{BTTV}/users/youtube/{channel_id}")),
        get::<SevenTvUser>(http, format!("{SEVEN_TV}/users/youtube/{channel_id}")),
    );
    merge(
        None,
        bttv.map(BttvUser::emotes),
        seven.and_then(|user| user.emote_set),
    )
}

async fn get<T: DeserializeOwned>(http: &reqwest::Client, url: String) -> Option<T> {
    let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
    response.json().await.ok()
}

/// Later sets win a shared code, so 7TV comes last.
fn merge(
    ffz: Option<FfzSets>,
    bttv: Option<Vec<BttvEmote>>,
    seven: Option<SevenTvSet>,
) -> Vec<Emote> {
    let ffz = ffz.into_iter().flat_map(FfzSets::into_emotes);
    let bttv = bttv.into_iter().flatten().map(BttvEmote::into_emote);
    let seven = seven
        .and_then(|set| set.emotes)
        .into_iter()
        .flatten()
        .filter_map(SevenTvEmote::into_emote);
    ffz.chain(bttv).chain(seven).collect()
}

#[derive(Deserialize)]
struct SevenTvUser {
    emote_set: Option<SevenTvSet>,
}

#[derive(Deserialize)]
struct SevenTvSet {
    emotes: Option<Vec<SevenTvEmote>>,
}

#[derive(Deserialize)]
struct SevenTvEmote {
    name: String,
    data: SevenTvData,
}

#[derive(Deserialize)]
struct SevenTvData {
    #[serde(default)]
    animated: bool,
    host: SevenTvHost,
}

#[derive(Deserialize)]
struct SevenTvHost {
    url: String,
    files: Vec<SevenTvFile>,
}

#[derive(Deserialize)]
struct SevenTvFile {
    name: String,
}

impl SevenTvEmote {
    fn into_emote(self) -> Option<Emote> {
        // AVIF is listed first but the image decoder has no AVIF support.
        let file = self
            .data
            .host
            .files
            .into_iter()
            .find(|file| file.name == "2x.webp")?;
        Some(Emote {
            code: self.name,
            url: format!("https:{}/{}", self.data.host.url, file.name),
            animated: self.data.animated,
            provider: EmoteProvider::SevenTv,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BttvUser {
    #[serde(default)]
    channel_emotes: Vec<BttvEmote>,
    #[serde(default)]
    shared_emotes: Vec<BttvEmote>,
}

impl BttvUser {
    fn emotes(mut self) -> Vec<BttvEmote> {
        self.channel_emotes.append(&mut self.shared_emotes);
        self.channel_emotes
    }
}

#[derive(Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
    #[serde(default)]
    animated: bool,
}

impl BttvEmote {
    fn into_emote(self) -> Emote {
        Emote {
            url: format!("https://cdn.betterttv.net/emote/{}/2x", self.id),
            code: self.code,
            animated: self.animated,
            provider: EmoteProvider::Bttv,
        }
    }
}

#[derive(Deserialize)]
struct FfzSets {
    sets: std::collections::HashMap<String, FfzSet>,
}

#[derive(Deserialize)]
struct FfzSet {
    emoticons: Vec<FfzEmote>,
}

#[derive(Deserialize)]
struct FfzEmote {
    name: String,
    urls: std::collections::HashMap<String, String>,
}

impl FfzSets {
    fn into_emotes(self) -> impl Iterator<Item = Emote> {
        self.sets
            .into_values()
            .flat_map(|set| set.emoticons)
            .filter_map(|emote| {
                let url = emote.urls.get("2").or(emote.urls.get("1"))?;
                Some(Emote {
                    url: if url.starts_with("//") {
                        format!("https:{url}")
                    } else {
                        url.clone()
                    },
                    code: emote.name,
                    animated: false,
                    provider: EmoteProvider::Ffz,
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_service_and_lets_7tv_win() {
        let ffz = serde_json::from_str(
            r#"{"sets":{"3":{"emoticons":[
                {"name":"ZrehplaR","urls":{"1":"https://cdn.ffz/1","2":"//cdn.ffz/2"}},
                {"name":"NoImages","urls":{}}]}}}"#,
        )
        .unwrap();
        let bttv = serde_json::from_str(
            r#"[{"id":"b1","code":"catJAM","imageType":"gif","animated":true}]"#,
        )
        .unwrap();
        let seven = serde_json::from_str(
            r#"{"emotes":[
                {"name":"catJAM","data":{"animated":true,"host":{"url":"//cdn.7tv.app/emote/s1",
                 "files":[{"name":"1x.avif"},{"name":"2x.webp"}]}}},
                {"name":"AvifOnly","data":{"host":{"url":"//cdn.7tv.app/emote/s2",
                 "files":[{"name":"2x.avif"}]}}}]}"#,
        )
        .unwrap();
        let emotes = merge(Some(ffz), Some(bttv), Some(seven));
        let summary: Vec<_> = emotes
            .iter()
            .map(|e| (e.code.as_str(), e.provider, e.url.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("ZrehplaR", EmoteProvider::Ffz, "https://cdn.ffz/2"),
                (
                    "catJAM",
                    EmoteProvider::Bttv,
                    "https://cdn.betterttv.net/emote/b1/2x"
                ),
                (
                    "catJAM",
                    EmoteProvider::SevenTv,
                    "https://cdn.7tv.app/emote/s1/2x.webp"
                ),
            ]
        );
    }

    #[test]
    fn a_user_without_an_emote_set_has_no_emotes() {
        let user: SevenTvUser = serde_json::from_str(r#"{"emote_set":null}"#).unwrap();
        assert!(merge(None, None, user.emote_set).is_empty());
    }
}

//! Two tongues. The audience is Austrian; the code is not.
//!
//! Short labels are fields, because they recur and must match between pages.
//! Prose that belongeth to one page alone is written out whole in each
//! language instead — a privacy notice assembled from forty fragments reads
//! like one, in either tongue.

use axum::http::HeaderMap;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};

pub const COOKIE: &str = "sg_lang";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
}

impl Lang {
    /// What they chose, else what their browser asketh for, else English.
    pub fn of(jar: &CookieJar, headers: &HeaderMap) -> Self {
        if let Some(chosen) = jar.get(COOKIE).map(|c| c.value().to_owned()) {
            return Self::from_code(&chosen);
        }
        headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok())
            .map_or(Self::En, |asked| {
                // "de-AT,de;q=0.9,en;q=0.8" — the first tag is enough.
                let first = asked.split(',').next().unwrap_or("").trim().to_ascii_lowercase();
                if first.starts_with("de") { Self::De } else { Self::En }
            })
    }

    pub fn from_code(code: &str) -> Self {
        if code.eq_ignore_ascii_case("de") { Self::De } else { Self::En }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::De => "de",
        }
    }

    /// The one they are not reading, for the toggle to offer.
    pub fn other(self) -> Self {
        match self {
            Self::En => Self::De,
            Self::De => Self::En,
        }
    }

    /// How the other tongue nameth itself, which is how a toggle should.
    pub fn own_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::De => "Deutsch",
        }
    }

    pub fn words(self) -> &'static Words {
        match self {
            Self::En => &ENGLISH,
            Self::De => &GERMAN,
        }
    }
}

/// Remembered for a year, and readable by script: it is a preference, not a
/// secret, and nothing turneth on it.
pub fn cookie_for(lang: Lang, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(COOKIE, lang.code());
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(secure);
    cookie.set_max_age(time::Duration::days(365));
    cookie
}

pub struct Words {
    pub sign_out: &'static str,
    pub footer: &'static str,
    pub privacy_link: &'static str,

    // Signing in
    pub welcome: &'static str,
    pub lede: &'static str,
    pub create_account: &'static str,
    pub sign_in: &'static str,
    pub sign_up: &'static str,
    pub email: &'static str,
    pub password: &'static str,
    pub password_ten: &'static str,
    pub point_cancelled: &'static str,
    pub point_exams: &'static str,
    pub point_nothing: &'static str,
    pub front_warning: &'static str,

    // The dashboard
    pub your_timetables: &'static str,
    pub one_link_each: &'static str,
    pub security_keys: &'static str,
    pub who_may_join: &'static str,
    pub how_it_fares: &'static str,
    pub google_note: &'static str,
    pub refused_title: &'static str,
    pub refused_body: &'static str,
    pub try_this_one: &'static str,
    pub untis_password: &'static str,
    pub lessons: &'static str,
    pub exams: &'static str,
    pub homework_count: &'static str,
    pub refreshed: &'static str,
    pub last_refresh_failed: &'static str,
    pub waiting_first: &'static str,
    pub state_well: &'static str,
    pub state_failing: &'static str,
    pub state_refused: &'static str,
    pub state_waiting: &'static str,
    pub subscribe_here: &'static str,
    pub show_qr: &'static str,
    pub qr_warning: &'static str,
    pub link_settings: &'static str,
    pub what_to_call_it: &'static str,
    pub ask_every: &'static str,
    pub refresh_note: &'static str,
    pub remind_before: &'static str,
    pub never: &'static str,
    pub only_exams_ring: &'static str,
    pub keep_cancelled: &'static str,
    pub carry_homework: &'static str,
    pub mark_holidays: &'static str,
    pub leave_out: &'static str,
    pub leave_out_note: &'static str,
    pub save: &'static str,
    pub new_address: &'static str,
    pub new_address_warning: &'static str,
    pub delete_link: &'static str,
    pub delete_link_warning: &'static str,
    pub new_link: &'static str,
    pub pushed_to_google: &'static str,
    pub disconnect_google: &'static str,
    pub push_to_google: &'static str,
    pub remove_school: &'static str,
    pub nothing_linked: &'static str,
    pub your_data: &'static str,
    pub your_data_note: &'static str,
    pub download_my_data: &'static str,
    pub delete_my_account: &'static str,
    pub delete_account_warning: &'static str,
    pub link_a_school: &'static str,
    pub webuntis_server: &'static str,
    pub school_login_name: &'static str,
    pub find_by_name: &'static str,
    pub or_from_url: &'static str,
    pub untis_username: &'static str,
    pub school_timezone: &'static str,
    pub link_school: &'static str,

    // Finding a school
    pub find_your_school: &'static str,
    pub find_lede: &'static str,
    pub school_or_town: &'static str,
    pub search: &'static str,
    pub nothing_found: &'static str,
    pub use_this_one: &'static str,
    pub three_letters: &'static str,

    // Waiting to be let in
    pub waiting_title: &'static str,
    pub waiting_body: &'static str,

    // Security keys
    pub key_lede: &'static str,
    pub registered: &'static str,
    pub never_finished: &'static str,
    pub no_key_yet: &'static str,
    pub register_a_key: &'static str,
    pub a_name_youll_know: &'static str,
    pub security_key: &'static str,
    pub register_this_one: &'static str,
    pub forget: &'static str,
    pub back_to_timetables: &'static str,
}

pub static ENGLISH: Words = Words {
    sign_out: "sign out",
    footer: "Your timetable, in your own calendar.",
    privacy_link: "What is kept, and why",

    welcome: "Your school timetable, in the calendar you already use",
    lede: "stundenglas reads your WebUntis timetable and publishes it as a private calendar \
           link. Google Calendar, iOS, Outlook and Thunderbird can all subscribe to it. \
           Cancelled lessons stay visible so you can see the free hour.",
    create_account: "Create an account",
    sign_in: "Sign in",
    sign_up: "Sign up",
    email: "Email",
    password: "Password",
    password_ten: "Password (ten characters or more)",
    point_cancelled: "Cancelled hours stay visible, so you can see the free period rather than \
                      wonder where it went.",
    point_exams: "Exams, homework and holidays come along, and an exam can remind you the day \
                  before.",
    point_nothing: "Nothing to install, and nothing to grant: one secret link your calendar \
                    subscribes to.",
    front_warning: "WebUntis gives students no way to grant access without a password, so this \
                    service has to store your school password to read your timetable. It is \
                    encrypted and the key is kept apart from the database — but know that \
                    before you hand it over, and check what your school's rules say:",

    your_timetables: "Your timetables",
    one_link_each: "One link per school. Each keeps its own clock.",
    security_keys: "Security keys",
    who_may_join: "Who may join",
    how_it_fares: "How it fares",
    google_note: "A subscribed link is refreshed on your calendar's own schedule — Google often \
                  takes hours. Connect Google to have changes written the moment we see them.",
    refused_title: "The school refused this login.",
    refused_body: " Nothing more will be tried until you enter the password again — repeating a \
                   refused password is how a WebUntis account gets locked.",
    try_this_one: "Try this one",
    untis_password: "WebUntis password",
    lessons: "lessons",
    exams: "exams",
    homework_count: "homework",
    refreshed: "refreshed",
    last_refresh_failed: "last refresh failed: ",
    waiting_first: "waiting for the first refresh",
    state_well: "up to date",
    state_failing: "not refreshing",
    state_refused: "needs your password",
    state_waiting: "waiting",
    subscribe_here: "Subscribe on this device",
    show_qr: "Show a QR code for a phone",
    qr_warning: "Point a camera at it. Treat it as you would the address itself: whoever scans \
                 it can read this timetable.",
    link_settings: "Settings for this link",
    what_to_call_it: "What to call it",
    ask_every: "Ask calendars to look again every",
    refresh_note: "A wish, not a rule. iOS honours it; Google refreshes on its own schedule \
                   whatever is asked.",
    remind_before: "Remind me before an exam",
    never: "Never",
    only_exams_ring: "Only exams ring. Ordinary lessons never do.",
    keep_cancelled: " Keep cancelled lessons, shown as free time",
    carry_homework: " Carry homework, on the day it is due",
    mark_holidays: " Mark the school's holidays",
    leave_out: "Leave out",
    leave_out_note: "Subjects ticked here are kept out of this link entirely — the lessons, \
                     their exams and their homework.",
    save: "Save",
    new_address: "New address",
    new_address_warning: "Draw a new address? Whatever is subscribed to the old one stops \
                          updating.",
    delete_link: "Delete link",
    delete_link_warning: "Delete this link? Anything subscribed to it stops updating.",
    new_link: "New link",
    pushed_to_google: "Pushed into Google Calendar as well as the link above.",
    disconnect_google: "Disconnect Google",
    push_to_google: "Push to Google Calendar",
    remove_school: "Remove school",
    nothing_linked: "No school linked yet. Add one below and a calendar link appears.",
    your_data: "Your data",
    your_data_note: "Everything here is yours to take or to be rid of. Deleting the account \
                     removes the schools, the calendar links and the stored passwords with it, \
                     at once and for good.",
    download_my_data: "Download my data",
    delete_my_account: "Delete my account",
    delete_account_warning: "Delete the account, the schools and every calendar link? This \
                             cannot be undone.",
    link_a_school: "Link a school",
    webuntis_server: "WebUntis server",
    school_login_name: "School login name",
    find_by_name: "Find your school by name",
    or_from_url: " — or take both from the URL webuntis.com sends you to, ",
    untis_username: "WebUntis username",
    school_timezone: "The school's timezone",
    link_school: "Link school",

    find_your_school: "Find your school",
    find_lede: "The same directory the WebUntis login page searches.",
    school_or_town: "School, or the town it is in",
    search: "Search",
    nothing_found: "Nothing found. The directory knows schools by their official name, which is \
                    not always the one people use — try the town instead.",
    use_this_one: "Use this one",
    three_letters: "Three letters or more, or the directory returns the world.",

    waiting_title: "Waiting to be let in",
    waiting_body: "Your account has been created. An administrator has to admit it before you \
                   can add a school.",

    key_lede: "A key is asked for after your password. It is a second factor, not a replacement: \
               this account service offers no passwordless sign-in yet.",
    registered: "registered",
    never_finished: "never finished",
    no_key_yet: "No key registered yet.",
    register_a_key: "Register a key",
    a_name_youll_know: "A name you will recognise",
    security_key: "Security key",
    register_this_one: "Register this device or key",
    forget: "Forget",
    back_to_timetables: "Back to your timetables",
};

pub static GERMAN: Words = Words {
    sign_out: "abmelden",
    footer: "Dein Stundenplan, in deinem eigenen Kalender.",
    privacy_link: "Was gespeichert wird, und warum",

    welcome: "Dein Stundenplan, im Kalender den du ohnehin verwendest",
    lede: "stundenglas liest deinen WebUntis-Stundenplan und veröffentlicht ihn als privaten \
           Kalender-Link. Google Calendar, iOS, Outlook und Thunderbird können ihn abonnieren. \
           Entfallene Stunden bleiben sichtbar, damit du die Freistunde siehst.",
    create_account: "Konto erstellen",
    sign_in: "Anmelden",
    sign_up: "Registrieren",
    email: "E-Mail",
    password: "Passwort",
    password_ten: "Passwort (mindestens zehn Zeichen)",
    point_cancelled: "Entfallene Stunden bleiben sichtbar — du siehst die Freistunde, statt dich \
                      zu wundern, wo sie hin ist.",
    point_exams: "Schularbeiten, Hausübungen und Ferien kommen mit, und eine Schularbeit kann \
                  dich am Vortag erinnern.",
    point_nothing: "Nichts zu installieren, nichts freizugeben: ein geheimer Link, den dein \
                    Kalender abonniert.",
    front_warning: "WebUntis bietet Schülerinnen und Schülern keine Freigabe ohne Passwort. \
                    Dieser Dienst muss dein Schulpasswort also speichern, um deinen Stundenplan \
                    zu lesen. Es ist verschlüsselt und der Schlüssel liegt getrennt von der \
                    Datenbank — aber das solltest du wissen, bevor du es aus der Hand gibst, \
                    und in der Hausordnung deiner Schule nachsehen:",

    your_timetables: "Deine Stundenpläne",
    one_link_each: "Ein Link je Schule. Jede behält ihre eigene Zeitzone.",
    security_keys: "Sicherheitsschlüssel",
    who_may_join: "Wer teilnehmen darf",
    how_it_fares: "Wie es läuft",
    google_note: "Ein abonnierter Link wird nach dem Zeitplan deines Kalenders aktualisiert — \
                  Google lässt sich oft Stunden Zeit. Verbinde Google, damit Änderungen sofort \
                  eingetragen werden.",
    refused_title: "Die Schule hat diese Anmeldung abgelehnt.",
    refused_body: " Es wird nichts weiter versucht, bis du das Passwort erneut eingibst — ein \
                   abgelehntes Passwort zu wiederholen ist der Weg, ein WebUntis-Konto sperren \
                   zu lassen.",
    try_this_one: "Dieses versuchen",
    untis_password: "WebUntis-Passwort",
    lessons: "Stunden",
    exams: "Schularbeiten",
    homework_count: "Hausübungen",
    refreshed: "aktualisiert",
    last_refresh_failed: "letzte Aktualisierung fehlgeschlagen: ",
    waiting_first: "wartet auf die erste Aktualisierung",
    state_well: "aktuell",
    state_failing: "aktualisiert nicht",
    state_refused: "braucht dein Passwort",
    state_waiting: "wartet",
    subscribe_here: "Auf diesem Gerät abonnieren",
    show_qr: "QR-Code fürs Handy zeigen",
    qr_warning: "Kamera darauf richten. Behandle ihn wie die Adresse selbst: wer ihn scannt, \
                 kann diesen Stundenplan lesen.",
    link_settings: "Einstellungen für diesen Link",
    what_to_call_it: "Bezeichnung",
    ask_every: "Kalender sollen nachsehen alle",
    refresh_note: "Ein Wunsch, keine Vorschrift. iOS hält sich daran; Google aktualisiert nach \
                   eigenem Zeitplan, was immer man bittet.",
    remind_before: "Vor einer Schularbeit erinnern",
    never: "Nie",
    only_exams_ring: "Nur Schularbeiten erinnern. Gewöhnliche Stunden nie.",
    keep_cancelled: " Entfallene Stunden behalten, als Freistunde angezeigt",
    carry_homework: " Hausübungen mitnehmen, am Tag der Fälligkeit",
    mark_holidays: " Ferien der Schule eintragen",
    leave_out: "Weglassen",
    leave_out_note: "Hier angehakte Fächer bleiben ganz aus diesem Link — die Stunden, ihre \
                     Schularbeiten und ihre Hausübungen.",
    save: "Speichern",
    new_address: "Neue Adresse",
    new_address_warning: "Neue Adresse erzeugen? Was die alte abonniert hat, wird nicht mehr \
                          aktualisiert.",
    delete_link: "Link löschen",
    delete_link_warning: "Diesen Link löschen? Was ihn abonniert hat, wird nicht mehr \
                          aktualisiert.",
    new_link: "Neuer Link",
    pushed_to_google: "Wird zusätzlich in Google Calendar eingetragen.",
    disconnect_google: "Google trennen",
    push_to_google: "In Google Calendar eintragen",
    remove_school: "Schule entfernen",
    nothing_linked: "Noch keine Schule verknüpft. Füge unten eine hinzu, dann erscheint ein \
                     Kalender-Link.",
    your_data: "Deine Daten",
    your_data_note: "Alles hier gehört dir — zum Mitnehmen oder zum Loswerden. Das Konto zu \
                     löschen entfernt die Schulen, die Kalender-Links und die gespeicherten \
                     Passwörter mit, sofort und endgültig.",
    download_my_data: "Meine Daten herunterladen",
    delete_my_account: "Mein Konto löschen",
    delete_account_warning: "Konto, Schulen und alle Kalender-Links löschen? Das lässt sich \
                             nicht rückgängig machen.",
    link_a_school: "Schule verknüpfen",
    webuntis_server: "WebUntis-Server",
    school_login_name: "Schulkürzel (login name)",
    find_by_name: "Schule nach Namen suchen",
    or_from_url: " — oder beides aus der Adresse nehmen, auf die webuntis.com weiterleitet, ",
    untis_username: "WebUntis-Benutzername",
    school_timezone: "Zeitzone der Schule",
    link_school: "Schule verknüpfen",

    find_your_school: "Schule suchen",
    find_lede: "Dasselbe Verzeichnis, in dem auch die WebUntis-Anmeldeseite sucht.",
    school_or_town: "Schule, oder der Ort",
    search: "Suchen",
    nothing_found: "Nichts gefunden. Das Verzeichnis kennt Schulen unter ihrem offiziellen \
                    Namen, der selten der gebräuchliche ist — versuch es mit dem Ort.",
    use_this_one: "Diese nehmen",
    three_letters: "Mindestens drei Buchstaben, sonst liefert das Verzeichnis die ganze Welt.",

    waiting_title: "Warten auf Freischaltung",
    waiting_body: "Dein Konto wurde angelegt. Eine Administratorin oder ein Administrator muss \
                   es freischalten, bevor du eine Schule hinzufügen kannst.",

    key_lede: "Ein Schlüssel wird nach dem Passwort verlangt. Er ist ein zweiter Faktor, kein \
               Ersatz: dieser Kontodienst bietet noch keine Anmeldung ohne Passwort.",
    registered: "registriert",
    never_finished: "nie abgeschlossen",
    no_key_yet: "Noch kein Schlüssel registriert.",
    register_a_key: "Schlüssel registrieren",
    a_name_youll_know: "Ein Name, den du wiedererkennst",
    security_key: "Sicherheitsschlüssel",
    register_this_one: "Dieses Gerät oder diesen Schlüssel registrieren",
    forget: "Vergessen",
    back_to_timetables: "Zurück zu deinen Stundenplänen",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_asking_for_german_gets_german() {
        let mut headers = HeaderMap::new();
        headers.insert("accept-language", "de-AT,de;q=0.9,en;q=0.8".parse().unwrap());
        assert_eq!(Lang::of(&CookieJar::new(), &headers), Lang::De);
    }

    #[test]
    fn a_browser_asking_for_anything_else_gets_english() {
        let mut headers = HeaderMap::new();
        headers.insert("accept-language", "fr-FR,fr;q=0.9".parse().unwrap());
        assert_eq!(Lang::of(&CookieJar::new(), &headers), Lang::En);
        assert_eq!(Lang::of(&CookieJar::new(), &HeaderMap::new()), Lang::En);
    }

    #[test]
    fn what_they_chose_outweighs_what_the_browser_asks() {
        let mut headers = HeaderMap::new();
        headers.insert("accept-language", "de-AT,de;q=0.9".parse().unwrap());
        let jar = CookieJar::new().add(Cookie::new(COOKIE, "en"));
        assert_eq!(Lang::of(&jar, &headers), Lang::En, "a chosen tongue must stick");
    }

    #[test]
    fn the_toggle_offers_the_other_one() {
        assert_eq!(Lang::En.other(), Lang::De);
        assert_eq!(Lang::De.other().own_name(), "English");
    }
}

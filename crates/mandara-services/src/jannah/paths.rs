//! Paths to Jannah: concrete deeds and manners (and a few things to avoid),
//! each backed only by verses and hadith in this dataset whose text says so.
//! `title` and `line` are plain encouragement, never scripture.

use mandara_i18n::tk;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathGroup {
    /// Prayer, fasting, remembrance, learning.
    Worship,
    /// Truthfulness, humility, mercy, fair dealing.
    Character,
    /// Parents, kin, children.
    Family,
    /// People and society.
    Others,
    /// Inner states: sincerity, patience, trust, hope.
    Heart,
    /// What keeps a person from Jannah.
    Avoid,
}

impl PathGroup {
    pub const ALL: [PathGroup; 6] =
        [PathGroup::Worship, PathGroup::Character, PathGroup::Family, PathGroup::Others, PathGroup::Heart, PathGroup::Avoid];

    /// The group's heading.
    pub fn title(self) -> &'static str {
        match self {
            PathGroup::Worship => tk("Worship"),
            PathGroup::Character => tk("Character"),
            PathGroup::Family => tk("Family and kin"),
            PathGroup::Others => tk("People around you"),
            PathGroup::Heart => tk("The heart"),
            PathGroup::Avoid => tk("Things to avoid"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Path {
    /// Stable id (for state files and settings).
    pub key: &'static str,
    pub group: PathGroup,
    /// 2 to 5 words; translate with `mandara_i18n::tr`.
    pub title: &'static str,
    /// One warm line of encouragement; translate with `mandara_i18n::tr`.
    pub line: &'static str,
    /// Ids of the verses (`quran-2:25`) and hadith (`bukhari-6094`,
    /// `muslim-2699a`) whose text says so.
    pub evidence: &'static [&'static str],
}

pub const PATHS: &[Path] = &[
    // ---------------------------------------------------------------- worship
    Path {
        key: "prayer-on-time",
        group: PathGroup::Worship,
        title: tk("Pray on time"),
        line: tk("Meet each prayer at its hour, dawn and afternoon most of all."),
        evidence: &["muslim-85b", "bukhari-574", "bukhari-1397", "muslim-14b"],
    },
    Path {
        key: "sunnah-prayers",
        group: PathGroup::Worship,
        title: tk("Add the sunnah prayers"),
        line: tk("A few extra rak'ahs each day quietly build you a home."),
        evidence: &["muslim-728a", "muslim-728c", "muslim-489"],
    },
    Path {
        key: "wudu",
        group: PathGroup::Worship,
        title: tk("Perfect your wudu"),
        line: tk("Wash with care, then pray two rak'ahs with your whole heart."),
        evidence: &["muslim-234a", "bukhari-1149", "muslim-2458"],
    },
    Path {
        key: "fasting",
        group: PathGroup::Worship,
        title: tk("Fast for His sake"),
        line: tk("Every fast you keep is remembered at a gate of its own."),
        evidence: &["bukhari-1896", "muslim-1152", "bukhari-3257", "bukhari-1898"],
    },
    Path {
        key: "mosque",
        group: PathGroup::Worship,
        title: tk("Walk to the mosque"),
        line: tk("Each walk to the mosque, morning or evening, is a welcome being prepared."),
        evidence: &["bukhari-662", "muslim-669"],
    },
    Path {
        key: "remembrance",
        group: PathGroup::Worship,
        title: tk("Remember Allah often"),
        line: tk("Fill quiet moments with His remembrance; it is a treasure kept for you."),
        evidence: &["bukhari-6384", "muslim-2704a", "bukhari-6408", "muslim-2689"],
    },
    Path {
        key: "forgiveness",
        group: PathGroup::Worship,
        title: tk("Seek forgiveness daily"),
        line: tk("Turn to Him morning and evening with a sincere plea for forgiveness."),
        evidence: &["bukhari-6306", "quran-3:133", "quran-3:136"],
    },
    Path {
        key: "knowledge",
        group: PathGroup::Worship,
        title: tk("Seek knowledge"),
        line: tk("Every step you take toward learning is a step on an easy road."),
        evidence: &["muslim-2699a"],
    },
    Path {
        key: "love-quran",
        group: PathGroup::Worship,
        title: tk("Love what you recite"),
        line: tk("Let your love for the Qur'an grow; that love is itself a way."),
        evidence: &["bukhari-774b"],
    },
    Path {
        key: "hajj",
        group: PathGroup::Worship,
        title: tk("Make an accepted Hajj"),
        line: tk("A pilgrimage done well and sincerely has a reward beyond measure."),
        evidence: &["bukhari-1773", "muslim-1349a"],
    },
    // -------------------------------------------------------------- character
    Path {
        key: "truthfulness",
        group: PathGroup::Character,
        title: tk("Speak the truth"),
        line: tk("Let your word be true, even when it costs you something."),
        evidence: &["bukhari-6094", "muslim-2607a", "muslim-2607c"],
    },
    Path {
        key: "tongue",
        group: PathGroup::Character,
        title: tk("Keep your tongue"),
        line: tk("Guard what you say and keep your desires within what is lawful."),
        evidence: &["bukhari-6474", "bukhari-6807"],
    },
    Path {
        key: "humility",
        group: PathGroup::Character,
        title: tk("Stay humble"),
        line: tk("Walk gently and think little of yourself; the humble are honoured there."),
        evidence: &["bukhari-4918", "bukhari-6071", "muslim-2853a"],
    },
    Path {
        key: "mercy",
        group: PathGroup::Character,
        title: tk("Be merciful to every creature"),
        line: tk("Even a drink of water for a thirsty animal is seen and rewarded."),
        evidence: &["bukhari-173", "muslim-2865a"],
    },
    Path {
        key: "easy-dealings",
        group: PathGroup::Character,
        title: tk("Be easy in your dealings"),
        line: tk("Give time to those who owe you, and let go when you can."),
        evidence: &["bukhari-3450", "muslim-1560c"],
    },
    Path {
        key: "justice",
        group: PathGroup::Character,
        title: tk("Be just with those in your care"),
        line: tk("If people are placed in your hands, serve them sincerely and fairly."),
        evidence: &["muslim-2865a"],
    },
    // ----------------------------------------------------------------- family
    Path {
        key: "parents",
        group: PathGroup::Family,
        title: tk("Honour your parents"),
        line: tk("Their old age is an open door; serve them with love while you can."),
        evidence: &["muslim-2551a", "muslim-2551b", "muslim-85b"],
    },
    Path {
        key: "kinship",
        group: PathGroup::Family,
        title: tk("Keep ties of kinship"),
        line: tk("Call, visit and forgive your relatives; never let the bond break."),
        evidence: &["bukhari-1396", "bukhari-5983", "muslim-13a"],
    },
    Path {
        key: "daughters",
        group: PathGroup::Family,
        title: tk("Cherish your children"),
        line: tk("Every kindness to your children counts, down to a shared date."),
        evidence: &["muslim-2630"],
    },
    Path {
        key: "loss-of-a-child",
        group: PathGroup::Family,
        title: tk("Patience when a child dies"),
        line: tk("If you lose a little one, your patient grief is not forgotten."),
        evidence: &["bukhari-1248", "bukhari-1381", "muslim-2632c"],
    },
    // ----------------------------------------------------------------- others
    Path {
        key: "salam",
        group: PathGroup::Others,
        title: tk("Spread salam"),
        line: tk("Greet everyone with peace; love grows from it, and faith with love."),
        evidence: &["muslim-54a"],
    },
    Path {
        key: "feed-and-visit",
        group: PathGroup::Others,
        title: tk("Feed and visit"),
        line: tk("Share your food with the needy and sit a while with the sick."),
        evidence: &["muslim-1028b", "muslim-1028", "muslim-2568a", "muslim-2568b"],
    },
    Path {
        key: "remove-harm",
        group: PathGroup::Others,
        title: tk("Remove harm from the road"),
        line: tk("Move the branch, the stone, the broken glass: small kindness, great reward."),
        evidence: &["muslim-1914c", "muslim-1914d", "muslim-1914e", "bukhari-2631"],
    },
    Path {
        key: "ease-hardship",
        group: PathGroup::Others,
        title: tk("Ease someone's hardship"),
        line: tk("Lighten a burden, cover a fault, help a brother or sister through."),
        evidence: &["muslim-2699a"],
    },
    Path {
        key: "orphans",
        group: PathGroup::Others,
        title: tk("Care for an orphan"),
        line: tk("Take an orphan under your wing and you will be close to the Prophet ﷺ."),
        evidence: &["bukhari-5304", "bukhari-6005", "muslim-2983"],
    },
    Path {
        key: "generosity",
        group: PathGroup::Others,
        title: tk("Give generously"),
        line: tk("Give, and give again, in every good cause you can reach."),
        evidence: &["bukhari-1897", "bukhari-2841", "bukhari-450"],
    },
    // ------------------------------------------------------------------ heart
    Path {
        key: "tawhid",
        group: PathGroup::Heart,
        title: tk("Worship Him alone"),
        line: tk("Hold to sincere faith in the One God; it is the root of every good."),
        evidence: &["bukhari-1237", "muslim-26a", "bukhari-3435"],
    },
    Path {
        key: "sincerity",
        group: PathGroup::Heart,
        title: tk("Act sincerely and steadily"),
        line: tk("Small good deeds, done sincerely and kept up, are the most beloved."),
        evidence: &["bukhari-6464", "bukhari-6467", "muslim-2818a"],
    },
    Path {
        key: "trust",
        group: PathGroup::Heart,
        title: tk("Trust in your Lord"),
        line: tk("Lean on Him rather than charms and omens, and walk on without fear."),
        evidence: &["bukhari-6472", "muslim-218b"],
    },
    Path {
        key: "patience",
        group: PathGroup::Heart,
        title: tk("Be patient in hardship"),
        line: tk("Illness, loss and long nights are not wasted when met with patience."),
        evidence: &["bukhari-5652", "bukhari-5653", "bukhari-6424", "quran-76:12"],
    },
    Path {
        key: "hope",
        group: PathGroup::Heart,
        title: tk("Hope in His mercy"),
        line: tk("His mercy is wider than any sin; never lose hope, and keep trying."),
        evidence: &["bukhari-6469", "muslim-2755", "bukhari-6467"],
    },
    Path {
        key: "awe",
        group: PathGroup::Heart,
        title: tk("Remember you will stand before Him"),
        line: tk("Let the thought of meeting your Lord keep you steady and kind."),
        evidence: &["quran-55:46"],
    },
    Path {
        key: "repentance",
        group: PathGroup::Heart,
        title: tk("Turn back to Him"),
        line: tk("Whatever has happened, sincere repentance opens the way again."),
        evidence: &["quran-66:8", "quran-19:60"],
    },
    // ------------------------------------------------------------------ avoid
    Path {
        key: "pride",
        group: PathGroup::Avoid,
        title: tk("Let go of pride"),
        line: tk("Pride, even a grain of it, stands in the way; choose humility."),
        evidence: &["muslim-91a", "muslim-91c"],
    },
    Path {
        key: "tale-bearing",
        group: PathGroup::Avoid,
        title: tk("Do not carry tales"),
        line: tk("Never pass words between people to set them against each other."),
        evidence: &["bukhari-6056", "muslim-105a"],
    },
    Path {
        key: "cutting-ties",
        group: PathGroup::Avoid,
        title: tk("Never cut family ties"),
        line: tk("However hard it is, do not be the one who breaks the bond."),
        evidence: &["bukhari-5984", "muslim-2556a"],
    },
    Path {
        key: "harming-neighbours",
        group: PathGroup::Avoid,
        title: tk("Never harm your neighbour"),
        line: tk("Let your neighbours feel safe from your words and your hands."),
        evidence: &["muslim-46"],
    },
    Path {
        key: "betraying-trust",
        group: PathGroup::Avoid,
        title: tk("Do not betray those in your care"),
        line: tk("Whoever you are responsible for, never deceive or neglect them."),
        evidence: &["bukhari-7151", "bukhari-7150", "muslim-142d"],
    },
    Path {
        key: "false-oaths",
        group: PathGroup::Avoid,
        title: tk("Never swear falsely"),
        line: tk("Do not take anything that is not yours with an oath, not even a twig."),
        evidence: &["muslim-137a"],
    },
];

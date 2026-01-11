# hecate's corpus organization strategy notes

The primary intent of the organization strategy is for the organization strategy itself to be an organic and changeable process.

With that in mind, the organization strategy breaks down into the following phases:

* Source File Acquisition (physical rips, bandcamp collection manager, etc)
* Archival-side storage by logical source
* (Auto)tagging/metadata correction
* Deploying to active Libraries via hard links
* Health-checking feedback loop

Deploying the library via hard links is the current critical paradigm for enabling the current organization strategy.
It allows us to organize our media logically by how it's acquired, which enables the following:

* material source is inherent to where it is organized, making integrity repairs/fills straightforward
* separates library organization by artist,album tags from acquisition sorting, making acquisition dedup a relative non-issue

It has some weaknesses:
* deduping across varying quality levels becomes trickier (e.g. spotify bulk webrip might have some tracks outclassed by flacs, but most will be present as OGGs)

That *should* be resolved by beets - I *should* be able to have beets idempotently deploy hard-links for the corpus to the library, and *should* be able to make it automatically select for bitrate in case of duplicates.
I should then also be able to have health-checking report on files that are in corpus but not in library - that *should* be indicative of files that are outclassed elsewhere in the corpus, and can thus be freed up.

---

This brings us back to the overall organization strategy. At a top level, the corpus is organized as:

* corpuss/{inbox,processed} - corpus files that need to be unpacked, corpus files that have been unpacked [retained for integrity repairs, ease of redistribution, catalogue rebuilding, etc]
* music/ - audio media
* rituals/ - automation for enforcing my will against my media

# Music organization

Music can come from a variety of sources. Non-exhaustively:
- bandcamp releases (usually flacs)
- cassette rips, vinyl recordings
- cd & audio blu-ray rips
- game soundtracks on steam

It makes the _most sense_ to differentiate first between media with physical backing sources, and media obtained from the web.

Past that - physical sources can just be organized loosely by the physical medium, and then whatever structure makes sense locally below that - typically a release name or identifier and then consistently named source file(s), e.g. "Side {1,2,3,4,...}" for vinyls, or perhaps CDs might be release/disknum/track.flac.

Physical media is unique, because we can (theoretically) guarantee we are only entering quality recordings/rips into the system. The riptuals I'll develop for various mediums should also have streamlined tagging input, and they're held to a high standard.

We have no such guarantees with web sources, and have to organize with the following in mind:

- some web sources will be lossy
- some web sources are more official than others
- indie musicians can occasionally be disorganized or inconsistent with their release strategies (<3)
- digital record labels themselves are inconsistent with their release strategies
  - some labels do not guarantee catalog numbers/IDs
- artists may sign with numerous labels during their lifetime, making this an unclear organization strategy for some bands.

Without the last constraint, the organization strategy would look roughly like:

- web
  - indie
    - bandcamp
      - Aviators
      - ...
    - msx
    - ...
  - labels
    - monstercat
      - MC001
      - MC002
      - ...
  - rips
    - spotify
  - legacy mp3?

However, this misses the forest for the trees: I want to have the monstercat releases organized like that, because it's (essentially) the only label where I both want most/all of the releases, and can acquire most/all of the releases in flacs reasonable.

That brings us to intent-based organizing based on the logical level we want to be collecting at:

- Label
- Artist (Albums/singles are identical to this for most artists)
- Bulk Collections

And secondarily, by acquisition methods:

- official downloads :)
- large rips etc
- one-off torrent type downloads

This gives us an organization strategy that looks like:

- music/
  - physical/
  - web/
    - rips
      - spotify [lossy]
      - nintendo
      - sonic
    - releases
      - bandcamp
      - monstercat
      - steam
      - nintendo
      - sonic
      - indie
        - neilcic
        - msx
    - torrents/
    - misc/
      - flac/
      - mp3/

The misc-folder is a load-bearing device. There will always be a long tail of odds and ends, and the best home we can give them is somewhere where they can all scatter broadly together. This is for one-off releases I've acquired, as well as old mp3's carried around, with the goal to eliminate as many misc mp3s as possible.
ADDENDUM TO BE EDITED IN LATER: I also own the same music in varying formats and digital distributors - e.g. flacs from nervous_testpilot or machinegirl bandcamp, mp3s from soundtracks on steam - necessitating Informed Decisions about what is already present when assimilating. hm.

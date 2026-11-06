The `hafs-258-*.json` files contain the ayah number and millisecond offsets
from MP3Quran's public timing API for Hafs read 258, captured on 6 November
2026. The source is
`https://www.mp3quran.net/api/v3/ayat_timing?surah=<number>&read=258`.
Page coordinates and polygons were omitted because audio playback does not
use them.

`seek-tone.mp3` is a generated one-second 440 Hz stereo tone at 44.1 kHz,
encoded as 64 kbit/s MP3 without Xing or ID3 headers. It contains no speech
or recording from a reciter. Decoder tests use it without opening a sound
device. It was generated with FFmpeg's sine source and libmp3lame; FFmpeg
is not needed to run the tests.

The other generated tones exercise a mono 22.05 kHz recording with Xing
encoder-delay metadata (`seek-mono-gapless.mp3`) and a 48 kHz stereo
recording (`seek-stereo-48k.mp3`). They use the same frequency, duration
and bitrate. The mono fixture includes an ID3 header; the stereo fixture
has neither ID3 nor Xing headers.

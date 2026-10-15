#!/bin/bash
# Builds the local test media the Phase 0 spike ranks against (not committed): speech clips with espeak-ng as 16 kHz
# mono WAV, two synthetic sounds, and a handful of photos from Wikimedia Commons (one matching each query).
#   evals/make-testbed.sh [out_dir]   (default evals/testbed)
set -euo pipefail
out=${1:-"$(dirname "$0")/testbed"}
mkdir -p "$out/img" "$out/audio"

say() {
  espeak-ng -v en-us -s 160 -w "$out/audio/tmp.wav" "$2"
  ffmpeg -loglevel error -y -i "$out/audio/tmp.wav" -ac 1 -ar 16000 -sample_fmt s16 "$out/audio/$1.wav"
  rm "$out/audio/tmp.wav"
}
say speech_sleep "I haven't been sleeping well lately. I keep waking up at three in the morning and can't fall back asleep. My doctor said to avoid screens before bed, keep the bedroom cool and dark, and try to go to sleep at the same time every night."
say speech_groceries "Don't forget to buy milk, eggs and bread on the way home tomorrow."
say speech_revenue "Our quarterly revenue grew by twenty percent, driven by strong sales in Europe."
say speech_flight_delay "Attention passengers. The flight to Tokyo has been delayed by two hours because of a storm."
say speech_cake_recipe "In this recipe we will bake a chocolate cake. First, preheat the oven to one hundred eighty degrees, then mix the flour, sugar and cocoa powder in a large bowl. Add the eggs, the melted butter and the milk, and stir until the batter is smooth."
say speech_car_repair "The mechanic said the brake pads are worn out and the front tyres need replacing before the inspection next week."
ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:duration=5" -ac 1 -ar 16000 -sample_fmt s16 "$out/audio/tone_440.wav"
ffmpeg -loglevel error -y -f lavfi -i "anoisesrc=d=5:c=pink:r=16000" -ac 1 -ar 16000 -sample_fmt s16 "$out/audio/noise_pink.wav"

# The first JPEG Commons' search returns for each phrase, at 1024 px. Check them: a search can return a colander
# for "bowl of pasta".
fetch() {
  local url
  url=$(curl -sG 'https://commons.wikimedia.org/w/api.php' -A 'omafind-testbed/0.1' \
    --data-urlencode "gsrsearch=$2 filetype:bitmap" --data-urlencode 'action=query' \
    --data-urlencode 'generator=search' --data-urlencode 'gsrnamespace=6' --data-urlencode 'gsrlimit=5' \
    --data-urlencode 'prop=imageinfo' --data-urlencode 'iiprop=url|mime' --data-urlencode 'iiurlwidth=1024' \
    --data-urlencode 'format=json' \
    | python3 -I -c 'import json,sys
pages = json.load(sys.stdin).get("query", {}).get("pages", {})
for page in sorted(pages.values(), key=lambda p: p.get("index", 99)):
    info = page["imageinfo"][0]
    if info.get("mime") == "image/jpeg":
        print(info["thumburl"]); break')
  [[ -n $url ]] || { echo "no image for: $2" >&2; return 1; }
  curl -sL -A 'omafind-testbed/0.1' -o "$out/img/$1.jpg" "$url"
  echo "$1.jpg ← $2"
}
fetch dog_beach "dog running on the beach"
fetch mountain_lake "mountain lake landscape"
fetch city_night "city skyline at night"
fetch spaghetti_plate "spaghetti bolognese plate"
fetch red_car "red sports car"
fetch office_desk "office desk computer"
fetch cat_sofa "cat sleeping on sofa"
echo "testbed in $out"

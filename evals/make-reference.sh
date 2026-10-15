#!/bin/bash
# Writes the fixed inputs of the golden-vector check into evals/reference: a screenshot-like PNG, a photo-like JPEG and
# a short speech WAV, all synthetic so they can live in git. `embed-spike golden evals/reference --write` then records
# the vectors the current engine makes for them; every engine update is checked against that file.
set -euo pipefail
out="$(dirname "$0")/reference"
mkdir -p "$out"

# A window-like screenshot: panels, bars of "text", a red alert box and a blue button (1440 × 900).
ffmpeg -loglevel error -y -f lavfi -i "color=c=0xf6f6f8:size=1440x900:rate=1" -frames:v 1 -vf "\
drawbox=x=0:y=0:w=1440:h=52:color=0xe6e6eb:t=fill,\
drawbox=x=0:y=52:w=240:h=848:color=0xeceef2:t=fill,\
drawbox=x=24:y=80:w=140:h=10:color=0x787882:t=fill,drawbox=x=24:y=110:w=90:h=10:color=0x787882:t=fill,\
drawbox=x=24:y=140:w=170:h=10:color=0x787882:t=fill,drawbox=x=24:y=170:w=110:h=10:color=0x787882:t=fill,\
drawbox=x=280:y=90:w=1100:h=60:color=0xffe4e4:t=fill,drawbox=x=300:y=112:w=520:h=14:color=0xc82828:t=fill,\
drawbox=x=280:y=180:w=900:h=10:color=0x3c3c46:t=fill,drawbox=x=280:y=206:w=1040:h=10:color=0x3c3c46:t=fill,\
drawbox=x=280:y=232:w=600:h=10:color=0x3c3c46:t=fill,drawbox=x=280:y=258:w=980:h=10:color=0x3c3c46:t=fill,\
drawbox=x=280:y=284:w=720:h=10:color=0x3c3c46:t=fill,drawbox=x=280:y=310:w=1000:h=10:color=0x3c3c46:t=fill,\
drawbox=x=280:y=620:w=160:h=40:color=0x007aff:t=fill,drawbox=x=310:y=636:w=100:h=8:color=white:t=fill" \
  "$out/screen.png"

# A photo-like picture: a sky gradient, a sun, two hills and a tree (1024 × 768, JPEG q≈90).
ffmpeg -loglevel error -y -f lavfi -i "gradients=size=1024x768:c0=0x5a96e6:c1=0xc8dcf0:x0=0:y0=0:x1=0:y1=767:rate=1" \
  -frames:v 1 -vf "\
drawbox=x=720:y=120:w=120:h=120:color=0xffe696:t=fill,\
drawbox=x=0:y=470:w=1024:h=298:color=0x5a9650:t=fill,drawbox=x=300:y=560:w=724:h=208:color=0x3c783c:t=fill,\
drawbox=x=160:y=420:w=80:h=120:color=0x1e5a28:t=fill,drawbox=x=192:y=540:w=16:h=60:color=0x6e5032:t=fill,\
noise=alls=8:allf=t" -q:v 6 "$out/photo.jpg"

# Four seconds of speech, 16 kHz mono 16-bit PCM, as the indexer writes audio windows.
espeak-ng -v en-us -s 160 -w "$out/tmp.wav" "The library closes at nine on weekdays and at five on Saturdays."
ffmpeg -loglevel error -y -i "$out/tmp.wav" -ac 1 -ar 16000 -sample_fmt s16 -t 4 "$out/speech.wav"
rm "$out/tmp.wav"
ls -l "$out"

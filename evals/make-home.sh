#!/bin/bash
# A fictional person's home folder for test runs and screenshots that look like real use: documents written here and
# converted to PDF/DOCX with LibreOffice, notes, the testbed photos under camera names (plus an edited copy for
# duplicate folding), screenshots with text for OCR, voice memos and videos, a small code folder, dates spread over
# a year and a half. Nothing in it is real. `just demo` runs the daemon and window with it as $HOME.
#   evals/make-home.sh OUT_DIR   (needs evals/testbed from `just testbed`, soffice, magick, espeak-ng, ffmpeg)
set -euo pipefail

H=${1:?usage: make-home.sh OUT_DIR}
REPO=$(cd "$(dirname "$0")/.." && pwd)
TB=$REPO/evals/testbed
[[ -d $TB/img ]] || { echo "evals/testbed is missing: run \`just testbed\` first" >&2; exit 1; }
FONT=$(fc-match -f '%{file}' 'Noto Sans')
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
rm -rf "$H"
mkdir -p "$H"/{Documents/{Housing,Finance,Work,Travel,Health},Notes/Recipes,Pictures/{2024/Algarve,2025/Camera,Screenshots},Downloads,Recordings,Videos,dev}

# --- documents -----------------------------------------------------------------------------------------------------
html() { # html NAME TITLE BODY → $WORK/NAME.html
  cat > "$WORK/$1.html" <<EOF
<!doctype html><html><head><meta charset="utf-8"><title>$2</title>
<style>body{font-family:'Liberation Serif',serif;font-size:11pt;line-height:1.45;margin:2cm}
h1{font-size:17pt}h2{font-size:12.5pt;margin-top:1.2em}table{border-collapse:collapse;width:100%}
td,th{border:1px solid #999;padding:4px 8px;text-align:left}.r{text-align:right}</style></head><body>
$3
</body></html>
EOF
}
convert_to() { # convert_to FORMAT NAME DEST
  local filter=$1
  # HTML opens in Writer/Web, which needs the Word filter named to write .docx.
  [[ $1 == docx ]] && filter='docx:MS Word 2007 XML'
  soffice -env:UserInstallation="file://$WORK/lo" --headless --convert-to "$filter" --outdir "$WORK" "$WORK/$2.html" > /dev/null 2>&1
  mv "$WORK/$2.$1" "$3"
}

html lease "Residential Lease Agreement" '
<h1>Residential Lease Agreement</h1>
<p>This agreement is made on 1 March 2024 between <b>Harbor Lane Properties Ltd.</b> ("the Landlord") and
<b>Sam Rivera</b> ("the Tenant") for the flat at 14 Calder Street, Apartment 3B.</p>
<h2>§ 1 Term</h2><p>The tenancy runs for twelve months from 1 March 2024 and continues month to month after that
unless either party gives sixty days written notice.</p>
<h2>§ 2 Rent</h2><p>Rent is 1,450 per month, due on the first day of each month by bank transfer to the account
named in Schedule A. Payments received more than ten days late incur a fee of five percent.</p>
<h2>§ 3 Security deposit</h2><p>The Tenant pays a security deposit equal to two months rent. The Landlord returns it
within thirty days of the end of the tenancy, less the cost of repairing any damage beyond ordinary wear.</p>
<h2>§ 4 Utilities</h2><p>The Tenant pays electricity, gas and internet. Water and refuse collection are included in
the rent.</p>
<div style="page-break-after:always"></div>
<h2>§ 13 Use of the premises</h2><p>The premises shall be used as a private residence only. No business shall be
operated from the premises without written consent of the Landlord.</p>
<h2>§ 14 Animals</h2><p>Tenant may keep one domestic pet with prior written consent of the Landlord. Consent shall not
be unreasonably withheld. A pet deposit of 300 is payable before the animal moves in and is returned with the
security deposit. This clause survives renewal of this lease.</p>
<p>Tenant is liable for any damage caused by animals kept on the premises, beyond ordinary wear.</p>
<h2>§ 15 Alterations</h2><p>No alterations, additions or improvements shall be made without prior written consent.
Small picture hooks are allowed; holes must be filled at the end of the tenancy.</p>
<h2>§ 16 Subletting</h2><p>The Tenant may not sublet the premises or any part of them, including short-term holiday
rentals, without the written consent of the Landlord.</p>
<div style="page-break-after:always"></div>
<h2>§ 21 Ending the tenancy</h2><p>At the end of the tenancy the Tenant returns all keys and leaves the flat clean. A
joint inspection is made on the last day, and both parties sign the inspection report.</p>
<p style="margin-top:3em">Signed: ______________________ (Landlord) &nbsp;&nbsp; ______________________ (Tenant)</p>'
convert_to pdf lease "$H/Documents/Housing/lease-2024.pdf"

html dentist "Invoice" '
<h1>Bright Smile Dental Clinic</h1><p>Dr. Amélie Moreau · 22 Orchard Road · Tel. 555 0142</p>
<p><b>Invoice 2025-0318</b> · 18 March 2025<br>Patient: Sam Rivera</p>
<table width="100%" border="1" cellpadding="4" cellspacing="0"><tr><th>Treatment</th><th class="r">Amount</th></tr>
<tr><td>Check-up and cleaning</td><td class="r">85.00</td></tr>
<tr><td>Composite filling, lower left molar</td><td class="r">140.00</td></tr>
<tr><td>Bitewing X-rays (2)</td><td class="r">45.00</td></tr>
<tr><th>Total</th><th class="r">270.00</th></tr></table>
<p>Please pay within 30 days. Insurance claim form attached; your plan covers 60% of the filling.</p>'
convert_to pdf dentist "$H/Documents/Finance/invoice-dentist-2025-03.pdf"

html utility "Energy bill" '
<h1>Northgrid Energy</h1><p>Account 7781-2290 · 14 Calder Street, Apt 3B</p>
<h2>Your bill for January–February 2025</h2>
<table width="100%" border="1" cellpadding="4" cellspacing="0"><tr><th>Item</th><th class="r">Amount</th></tr>
<tr><td>Electricity, 412 kWh</td><td class="r">98.40</td></tr><tr><td>Gas, 96 m³</td><td class="r">71.15</td></tr>
<tr><td>Standing charges</td><td class="r">18.60</td></tr><tr><th>Total due 15 March</th><th class="r">188.15</th></tr></table>
<p>Your usage was 12% higher than the same period last year, mostly heating during the cold snap in February.</p>'
convert_to pdf utility "$H/Documents/Housing/energy-bill-2025-02.pdf"

html offer "Offer letter" '
<h1>Kestrel Labs</h1><p>8 September 2025</p><p>Dear Sam,</p>
<p>We are delighted to offer you the position of <b>Senior Product Designer</b>, starting 3 November 2025. Your
annual salary will be 92,000, paid monthly, with 28 days of paid holiday and a yearly learning budget of 1,500.</p>
<p>You will work with the search team on the desktop app, reporting to Priya Natarajan. The first three months are a
probation period with a two-week notice period on either side.</p>
<p>Please sign and return this letter by 19 September.</p><p>Warm regards,<br>Jonas Weber, Head of People</p>'
convert_to pdf offer "$H/Documents/Work/offer-letter-kestrel.pdf"

html roadmap "Q4 roadmap" '
<h1>Search app: Q4 roadmap</h1><p>Draft for the planning meeting on 12 September.</p>
<h2>Goals</h2><ul><li>Results in under 50 ms at 300k files</li><li>Find photos by what is in them</li>
<li>Onboarding that finishes indexing a typical laptop in one evening</li></ul>
<h2>Milestones</h2><table width="100%" border="1" cellpadding="4" cellspacing="0"><tr><th>When</th><th>What</th></tr>
<tr><td>October</td><td>New result list and preview pane; keyboard-first actions</td></tr>
<tr><td>November</td><td>Near-duplicate photo grouping; date filters like "last week"</td></tr>
<tr><td>December</td><td>Code search with whole functions; beta for 200 users</td></tr></table>
<h2>Risks</h2><p>GPU memory on older laptops; battery use while indexing; people distrusting a tool that reads
all their files, so everything stays on the device.</p>'
convert_to docx roadmap "$H/Documents/Work/q4-roadmap.docx"

html sleep "Sleep study" '
<h1>Riverside Sleep Clinic — Study summary</h1><p>Patient: Sam Rivera · Night of 4 June 2025</p>
<p>Total sleep time 6 h 12 min, sleep efficiency 81%. Twelve awakenings, most in the second half of the night.
No significant sleep apnea (AHI 3.1). Deep sleep 14%, REM 19%.</p>
<p><b>Recommendations:</b> keep a consistent wake time, including weekends; no caffeine after 2 pm; screens off an hour
before bed; review in three months.</p>'
convert_to pdf sleep "$H/Documents/Health/sleep-study-summary.pdf"

html boarding "Boarding pass" '
<h1>Boarding pass</h1><p><b>RIVERA / SAM</b></p>
<p>Flight <b>TP 1352</b> · Amsterdam (AMS) → Lisbon (LIS)<br>Date 17 Apr 2025 · Boarding 06:35 · Departure 07:05<br>
Seat <b>14C</b> · Group 3</p>
<p>Gate closes 20 minutes before departure. One cabin bag up to 8 kg.</p>'
convert_to pdf boarding "$H/Documents/Travel/boarding-pass-TP1352.pdf"

html rules "House rules" '
<h1>House rules — 14 Calder Street</h1><ul>
<li>Quiet hours from 22:00 to 7:00 on weekdays, 23:00 to 9:00 at weekends.</li>
<li>Bikes go in the basement rack, not the hallway.</li><li>Pets on leads in shared areas.</li>
<li>Recycling is collected on Tuesdays; glass goes to the bank on the corner.</li>
<li>Report leaks to the caretaker, Marek, on 555 0199.</li></ul>'
convert_to pdf rules "$H/Downloads/House rules.pdf"

cat > "$H/Documents/Housing/move-in-checklist.md" <<'EOF'
# Moving in — 14 Calder St

- [x] Sign the lease and pay the deposit
- [x] Book the van for 1 March, 8am
- [ ] Ask the landlord about the pet deposit before Miso moves in
- [ ] Register with Northgrid for electricity and gas
- [ ] Change address at the bank, the dentist and the insurer
- [ ] Measure the alcove for the bookshelf (about 92 cm?)
- [ ] Find out which day recycling goes out
EOF

cat > "$H/Documents/Finance/budget-2025.csv" <<'EOF'
month,rent,groceries,energy,transport,eating_out,savings
January,1450,412.30,94.10,86.00,138.50,600
February,1450,389.75,94.05,86.00,92.00,600
March,1450,441.20,71.40,104.50,210.75,450
April,1450,398.60,58.90,86.00,320.00,300
May,1450,405.15,41.30,86.00,145.25,650
EOF

cat > "$H/Documents/Finance/tax-notes-2024.md" <<'EOF'
# Tax return 2024 — notes

Deadline is 31 May. Things to collect:

1. Annual salary statement from Kestrel (arrives in February)
2. Dentist invoices for the medical deduction — March and November
3. Receipts for the home office chair and monitor (bought in August)
4. Interest statement from the savings account

The accountant said working from home three days a week counts if I keep a calendar of office days.
EOF

cat > "$H/Documents/Work/meeting-notes-2025-09-12.md" <<'EOF'
# Planning meeting — 12 Sep 2025

Present: Priya, Jonas, Lea, Sam

## Decisions
- Ship the new result list in October; the old one stays behind a flag for two weeks.
- Photo grouping threshold stays at 0.88 until we have more data from the beta.

## Open questions
- How do we explain on-device indexing in onboarding without scaring people?
- Lea worries indexing drains the battery; we pause on battery by default.

## Actions
- Sam: mock up the empty state and the first-run screen by Friday.
- Priya: write the beta invite email.
EOF

cat > "$H/Documents/Travel/lisbon-itinerary.md" <<'EOF'
# Lisbon, 17–23 April

**Thu 17** — land 09:10, tram 28 to Graça, check in at the guesthouse on Rua da Voz do Operário.
**Fri 18** — Belém: the monastery early before the queues, pastéis de nata, MAAT in the afternoon.
**Sat 19** — day trip to Sintra; train from Rossio at 09:11. Pena Palace tickets booked for 11:00.
**Sun 20** — LX Factory market, sunset at Miradouro da Senhora do Monte.
**Mon 21** — surf lesson in Carcavelos, 10:00–12:00 (bring a towel).
**Tue 22** — Gulbenkian museum, dinner with Inês at a tasca in Alfama.
**Wed 23** — fly home 18:40.
EOF

cat > "$H/Notes/Recipes/chocolate-cake.md" <<'EOF'
# Grandma's chocolate cake

Serves 8. Oven at 180 °C, 23 cm tin.

- 200 g dark chocolate, 200 g butter
- 4 eggs, 180 g sugar
- 120 g flour, a pinch of salt, 1 tsp baking powder

Melt the chocolate and butter together. Whisk the eggs and sugar until pale and doubled, fold in the chocolate,
then the flour. Bake 35 minutes — the middle should still wobble slightly. Let it cool in the tin before turning out.
EOF

cat > "$H/Notes/Recipes/weeknight-dal.md" <<'EOF'
# Weeknight dal

Red lentils (250 g) rinsed, simmered 20 min with turmeric and a bay leaf. Meanwhile fry an onion, three garlic
cloves, a thumb of ginger and cumin seeds in ghee until golden. Stir into the lentils with a tin of tomatoes and a
squeeze of lime. Coriander on top, rice or flatbread on the side.
EOF

cat > "$H/Notes/reading-list.md" <<'EOF'
# Reading list

- *The Design of Everyday Things* — Don Norman (re-read the chapter on mappings)
- *Why We Sleep* — Matthew Walker, recommended by the sleep clinic
- *Piranesi* — Susanna Clarke
- *A Pattern Language* — the bits about light on two sides of every room
EOF

cat > "$H/Notes/journal-2025-10.md" <<'EOF'
# October

**3rd** — First week at Kestrel nearly done. The search team is small and kind. Rode the bike in every day.
**9th** — Miso knocked the plant off the windowsill again. Moved it to the bookshelf.
**12th** — Long call with mum about the trip to the Algarve next summer; she wants to see the beach where we took
the photo of the dog running.
**14th** — Still waking up around 4 am. Trying the no-screens-after-ten rule from the clinic.
EOF

cp "$TB/docs/support-log.txt" "$H/Downloads/support-chat-export.txt"

# --- pictures ------------------------------------------------------------------------------------------------------
cp "$TB/img/dog_beach.jpg" "$H/Pictures/2024/Algarve/IMG_4471.jpg"
magick "$TB/img/dog_beach.jpg" -gravity center -crop 80%x80%+0+0 +repage -modulate 104,110 "$H/Pictures/2024/Algarve/IMG_4471-edited.jpg"
cp "$TB/img/mountain_lake.jpg" "$H/Pictures/2024/Algarve/IMG_4502.jpg"
cp "$TB/img/cat_sofa.jpg" "$H/Pictures/2025/Camera/IMG_2201.jpg"
cp "$TB/img/spaghetti_plate.jpg" "$H/Pictures/2025/Camera/IMG_2236.jpg"
cp "$TB/img/red_car.jpg" "$H/Pictures/2025/Camera/IMG_2290.jpg"
cp "$TB/img/city_night.jpg" "$H/Pictures/2025/Camera/IMG_2315.jpg"
cp "$TB/img/office_desk.jpg" "$H/Pictures/2025/Camera/IMG_2347.jpg"
# Phone photos are mostly portrait: crops of the testbed shots, so tall pictures sit beside wide ones.
magick "$TB/img/city_night.jpg" -gravity center -crop 56%x100%+0+0 +repage "$H/Pictures/2025/Camera/IMG_2318.jpg"
magick "$TB/img/mountain_lake.jpg" -gravity center -crop 50%x100%+0+0 +repage "$H/Pictures/2024/Algarve/IMG_4510.jpg"
magick "$TB/img/spaghetti_plate.jpg" -gravity center -crop 60%x100%+0+0 +repage "$H/Pictures/2025/Camera/IMG_2240.jpg"
magick "$TB/img/cat_sofa.jpg" -gravity center -crop 100%x70%+0+0 +repage -resize 160%x100%! "$H/Pictures/2025/Camera/PANO_2205.jpg"

shot() { # shot NAME TEXT: a dark app-window screenshot with TEXT in it
  magick -size 1280x720 xc:'#1a1b26' \
    -fill '#24283b' -draw 'roundrectangle 160,110 1120,610 14,14' \
    -font "$FONT" -fill '#c0caf5' -pointsize 30 -annotate +210+190 "$2" \
    "$H/Pictures/Screenshots/$1"
}
shot "Screenshot 2025-10-02 14-31-07.png" $'Order confirmed\n\nOrder #A-20931  ·  2 items\nOak bookshelf, 90 cm      €179.00\nDesk lamp, brass          €42.50\n\nDelivery Thursday 9 October, 8:00–12:00'
shot "Screenshot 2025-09-28 09-12-44.png" $'Flight TP 1352 is delayed\n\nNew departure 08:20 (was 07:05)\nGate D7  ·  Amsterdam → Lisbon\n\nWe are sorry for the inconvenience.'
shot "Screenshot 2025-10-10 22-47-15.png" $'Error: connection refused\n\nCould not reach the sync server at\nsync.kestrel.dev:443\n\nCheck your VPN and try again.'
shot "Screenshot 2025-08-19 18-03-52.png" $'Inês: are we still on for dinner tuesday?\nSam: yes! 8pm at the tasca in Alfama\nInês: perfect, I booked a table for 3\nSam: bringing Lea too'

# --- recordings and videos -----------------------------------------------------------------------------------------
memo() { # memo NAME TEXT
  espeak-ng -v en-us -s 150 -w "$WORK/memo.wav" "$2"
  ffmpeg -loglevel error -y -i "$WORK/memo.wav" -c:a aac -b:a 64k "$H/Recordings/$1"
}
memo "voice-memo-2025-03-12.m4a" "Reminder: call the landlord about the pet deposit before Miso moves in. Also ask whether the bike can go in the basement."
memo "voice-memo-2025-06-05.m4a" "Notes from the sleep clinic. Keep the same wake up time every day, no coffee after two, and no screens an hour before bed. Follow up in September."
memo "voice-memo-2025-09-30.m4a" "Groceries for the weekend: red lentils, ginger, garlic, two tins of tomatoes, coriander, limes and dark chocolate for the cake."
ffmpeg -loglevel error -y -i "$TB/audio/speech_flight_delay.wav" -c:a aac -b:a 64k "$H/Recordings/airport-announcement.m4a"
ffmpeg -loglevel error -y -i "$TB/audio/speech_car_repair.wav" -c:a aac -b:a 64k "$H/Recordings/garage-call-2025-05-02.m4a"

# A trip video: the Algarve photos as a slideshow with a spoken caption.
espeak-ng -v en-us -s 145 -w "$WORK/trip.wav" "Day three in the Algarve. The dog would not stop running on the beach. Tomorrow we drive up to the lake in the mountains."
ffmpeg -loglevel error -y \
  -loop 1 -t 5 -i "$TB/img/dog_beach.jpg" -loop 1 -t 5 -i "$TB/img/mountain_lake.jpg" -i "$WORK/trip.wav" \
  -filter_complex "[0]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,setsar=1[a];[1]scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,setsar=1[b];[a][b]concat=n=2:v=1:a=0,format=yuv420p[v]" \
  -map "[v]" -map 2:a -c:v libx264 -preset veryfast -c:a aac -shortest "$H/Videos/algarve-day-3.mp4"
# A talk recording: a dark frame with the sleep speech.
ffmpeg -loglevel error -y -f lavfi -i color=c=0x1e2030:s=1280x720:d=30 -i "$TB/audio/speech_sleep.wav" \
  -vf "drawtext=font='Noto Sans':text='Sleep and memory — lecture 4':fontcolor=0xc0caf5:fontsize=44:x=(w-tw)/2:y=(h-th)/2" \
  -c:v libx264 -preset veryfast -c:a aac -shortest "$H/Videos/sleep-lecture-4.mp4"

# --- code ----------------------------------------------------------------------------------------------------------
mkdir -p "$H/dev/kestrel-sync/src" "$H/dev/weather-widget/src"
cp "$REPO/crates/omafind-client/src/lib.rs" "$H/dev/kestrel-sync/src/client.rs"
cp "$REPO/crates/omafind-setup/src/download.rs" "$H/dev/kestrel-sync/src/download.rs"
cat > "$H/dev/weather-widget/src/forecast.ts" <<'EOF'
// Fetches the hourly forecast and retries with exponential backoff when the API is rate limited.
export interface Hour { time: string; tempC: number; rainMm: number }

const MAX_BACKOFF_MS = 8_000;

export async function forecast(lat: number, lon: number, attempts = 5): Promise<Hour[]> {
  let delay = 250;
  for (let attempt = 0; attempt < attempts; attempt++) {
    const res = await fetch(`https://api.example.com/v1/forecast?lat=${lat}&lon=${lon}&hourly=temp,rain`);
    if (res.status === 429 && attempt + 1 < attempts) {
      await new Promise((r) => setTimeout(r, delay));
      delay = Math.min(delay * 2, MAX_BACKOFF_MS);
      continue;
    }
    if (!res.ok) throw new Error(`forecast failed: ${res.status}`);
    const body = await res.json();
    return body.hourly.time.map((time: string, i: number) => ({
      time,
      tempC: body.hourly.temp[i],
      rainMm: body.hourly.rain[i],
    }));
  }
  throw new Error("forecast: out of attempts");
}

/** Whether to show the umbrella icon: any rain above half a millimetre in the next six hours. */
export function needsUmbrella(hours: Hour[]): boolean {
  return hours.slice(0, 6).some((h) => h.rainMm > 0.5);
}
EOF

# --- dates: spread over the last year and a half, like real files ----------------------------------------------------
stamp() { touch -d "$1" "$H/$2"; }
stamp "2024-03-01 10:12" "Documents/Housing/lease-2024.pdf"
stamp "2024-02-20 21:40" "Documents/Housing/move-in-checklist.md"
stamp "2025-03-02 08:05" "Documents/Housing/energy-bill-2025-02.pdf"
stamp "2025-03-18 16:22" "Documents/Finance/invoice-dentist-2025-03.pdf"
stamp "2025-05-31 19:10" "Documents/Finance/budget-2025.csv"
stamp "2025-02-11 20:30" "Documents/Finance/tax-notes-2024.md"
stamp "2025-09-08 11:45" "Documents/Work/offer-letter-kestrel.pdf"
stamp "2025-09-10 17:20" "Documents/Work/q4-roadmap.docx"
stamp "2025-09-12 15:58" "Documents/Work/meeting-notes-2025-09-12.md"
stamp "2025-04-02 22:14" "Documents/Travel/lisbon-itinerary.md"
stamp "2025-04-16 18:30" "Documents/Travel/boarding-pass-TP1352.pdf"
stamp "2025-06-05 09:40" "Documents/Health/sleep-study-summary.pdf"
stamp "2024-03-03 13:00" "Downloads/House rules.pdf"
stamp "2024-08-14 18:52" "Pictures/2024/Algarve/IMG_4471.jpg"
stamp "2024-08-15 10:20" "Pictures/2024/Algarve/IMG_4471-edited.jpg"
stamp "2024-08-16 07:31" "Pictures/2024/Algarve/IMG_4502.jpg"
stamp "2024-08-16 20:05" "Videos/algarve-day-3.mp4"
stamp "2025-01-19 15:44" "Pictures/2025/Camera/IMG_2201.jpg"
stamp "2025-03-30 20:12" "Pictures/2025/Camera/IMG_2236.jpg"
stamp "2025-05-02 12:03" "Pictures/2025/Camera/IMG_2290.jpg"
stamp "2025-07-21 23:18" "Pictures/2025/Camera/IMG_2315.jpg"
stamp "2025-09-01 09:02" "Pictures/2025/Camera/IMG_2347.jpg"
stamp "2025-07-21 23:31" "Pictures/2025/Camera/IMG_2318.jpg"
stamp "2024-08-17 08:12" "Pictures/2024/Algarve/IMG_4510.jpg"
stamp "2025-03-30 20:15" "Pictures/2025/Camera/IMG_2240.jpg"
stamp "2025-01-19 15:52" "Pictures/2025/Camera/PANO_2205.jpg"
stamp "2025-03-12 08:15" "Recordings/voice-memo-2025-03-12.m4a"
stamp "2025-06-05 10:02" "Recordings/voice-memo-2025-06-05.m4a"
stamp "2025-09-30 18:44" "Recordings/voice-memo-2025-09-30.m4a"
stamp "2025-04-17 07:02" "Recordings/airport-announcement.m4a"
stamp "2025-05-02 11:30" "Recordings/garage-call-2025-05-02.m4a"
stamp "2025-02-24 19:00" "Videos/sleep-lecture-4.mp4"

# --- what the app needs from the real home ----------------------------------------------------------------------------
mkdir -p "$H/.local/state" "$H/.local/share" "$H/.config/omafind"
# The window takes its colors from the real Omarchy theme and its fonts from the real user fonts.
[[ -d $HOME/.local/state/omarchy ]] && ln -s "$HOME/.local/state/omarchy" "$H/.local/state/omarchy"
[[ -d $HOME/.local/share/fonts ]] && ln -s "$HOME/.local/share/fonts" "$H/.local/share/fonts"
printf '[index]\nfolders = ["%s", "%s", "%s", "%s", "%s", "%s"]\ncode = ["%s"]\n' \
  "$H/Documents" "$H/Notes" "$H/Pictures" "$H/Downloads" "$H/Recordings" "$H/Videos" "$H/dev" \
  > "$H/.config/omafind/config.toml"
echo "made $H: $(find "$H" -type f -not -path '*/.config/*' | wc -l) files"

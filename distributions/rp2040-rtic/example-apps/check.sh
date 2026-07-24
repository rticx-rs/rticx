#!/bin/bash
# ./check.sh <cargo flags(e.g. --release)>

flags=$1
: ${iterations:=1} # by default one iteration

applications=("mmrtic_led_toggler" "rticv1_led_toggler" "rticv2_led_toggler" "mmrtic_multitasker" "rticv1_multitasker" "rticv2_multitasker")
bins=("led_toggler_mmrtic"  "led_toggler_rticv1" "led_toggler_rticv2" "multitasker_mmrtic" "multitasker_rticv1" "multitasker_rticv2" )

for app in ${applications[@]}; do
  echo "App: $app"
  cd $app 
  cargo build $flags || exit 1
  cd -
done


#!/bin/bash

applications=("mmrtic_led_toggler" "rticv1_led_toggler" "rticv2_led_toggler" "mmrtic_multitasker" "rticv1_multitasker" "rticv2_multitasker")

echo DEBUG
for app in ${applications[@]}; do
   echo "-------------------------- $app ---------------------------"
   cd $app
   cargo size -q
   cd -
done


echo RELEASE
for app in ${applications[@]}; do
   echo "-------------------------- $app ---------------------------"
   cd $app
   cargo size --release -q
   cd -
done
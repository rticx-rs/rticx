#!/bin/bash
# ./run_timings.sh <iterations> <cargo flags(e.g. --release)>


iterations=$1
flags=$2
: ${iterations:=1} # by default one iteration

rm -rf timings
mkdir timings
applications=("mmrtic_led_toggler" "rticv1_led_toggler" "rticv2_led_toggler" "mmrtic_multitasker" "rticv1_multitasker" "rticv2_multitasker")
bins=("led_toggler_mmrtic"  "led_toggler_rticv1" "led_toggler_rticv2" "multitasker_mmrtic" "multitasker_rticv1" "multitasker_rticv2" )

for (( i=1; i<=$iterations; i++ ))
do
   for app in ${applications[@]}; do
      echo "Iteration $i of $app"
      cd $app 
      cargo clean &> /dev/null
      cargo build $flags --timings &> /dev/null
      cd -
      cp $app/target/cargo-timings/cargo-timing.html timings/${app}_timing_${i}.html
   done
   sleep 10
done

for app in ${applications[@]}; do
   echo -n "${app}: "
   for (( i=1; i<=$iterations; i++ ))
   do
      app_file=timings/${app}_timing_${i}.html
      timing=$(grep '<td>Total time:</td><td>' $app_file | awk -F'<td>|</td>' '{print $4}')
      echo -n " ${timing} "
   done
   echo
done

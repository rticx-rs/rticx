SECTIONS 
{
  .shared : ALIGN(4)
  {
    KEEP(microamp-data.o(.shared));
    . = ALIGN(4);
  } > SHARED
}

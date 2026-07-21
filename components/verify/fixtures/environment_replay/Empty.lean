/-
Fixture: a module that declares nothing. Compiling it yields a valid `.olean` with zero
constants.

This is the zero-declaration guard's test case, and it is the fail-open shape we care
most about: a checker handed this file must NOT report "nothing was wrong, therefore
clean". Upstream SafeVerify prints "Finished with no errors." here. Our replay must exit
non-zero with the error marker and emit no summary line.
-/

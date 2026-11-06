use bootconfig::merge_parameters;
use test_case::test_case;

#[test_case(
    "kernel.fips = 1\nkernel.fips = 0\n",
    "kernel.fips = 0\n"
    ; "the_last_assignment_of_a_repeated_key_wins"
)]
#[test_case(
    "kernel.x = \x0ca, b\x0b\n",
    "kernel.x = a, b\n"
    ; "the_edges_the_kernel_strips_are_dropped"
)]
#[test_case(
    "kernel.module_blacklist = i8042; init.extra = value\n",
    "init.extra = value\nkernel.module_blacklist = i8042\n"
    ; "a_shared_line_keeps_both_assignments"
)]
#[test_case(
    "kernel.nosmt\nkernel.fips = 1\n",
    "kernel.fips = 1\nkernel.nosmt\n"
    ; "a_flag_is_kept_without_a_value"
)]
#[test_case("", "" ; "nothing_to_merge")]
fn test_merge(input: &str, expected: &str) {
    let merged = merge_parameters(input).unwrap();
    assert_eq!(merged, expected);
    assert_eq!(merge_parameters(&merged).unwrap(), merged);
}

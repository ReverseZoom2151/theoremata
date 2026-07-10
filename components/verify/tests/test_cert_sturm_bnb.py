from theoremata_tools.cert_bnb import check as check_bnb, export_bnb_cert, leaf_pass
from theoremata_tools.cert_sturm import check as check_sturm, export_sturm_cert


def test_sturm_root_count_round_trip_and_tamper_rejection():
    log = export_sturm_cert([-1, 0, 1], [-2, 2])
    assert check_sturm(log)["valid"] is True
    log["steps"][-1]["op"] = "assert_chain"
    assert check_sturm(log)["valid"] is False


def test_branch_and_bound_pass_leaf_round_trip():
    log = export_bnb_cert(
        ["const", "1"],
        ["x"],
        [["0", "1"]],
        leaf_pass([["0", "1"]]),
    )
    assert check_bnb(log)["valid"] is True
    log["steps"][1]["root"]["box"] = [["0", "2"]]
    assert check_bnb(log)["valid"] is False

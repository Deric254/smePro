use super::common::*;

#[test]
fn test_moving_average_forecast_rejects_zero_window() {
    // THE BUG THIS FIXES: window=0 used to slice the recent-values
    // window down to empty and then divide its (empty) sum by its own
    // zero length — a silent NaN that serde_json's own Serialize impl
    // turns into a bare JSON `null` at the HTTP boundary, with no
    // error anywhere in the chain. A caller has no way to distinguish
    // "no data yet" from "you asked for a nonsensical window" once
    // that null reaches them. This proves the guard added in
    // forecast.rs rejects it outright instead, before any division
    // happens.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "FCAST-001", "Forecast Item", 50, 100, 200);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let err = crate::forecast::moving_average_forecast(&conn, &biz, &uid, "sales", "revenue", "month", 0)
        .unwrap_err();
    assert!(err.to_string().contains("window"), "unexpected error: {err}");
}

#[test]
fn test_moving_average_forecast_accepts_a_real_window() {
    // Sanity check alongside the zero-window rejection above: a normal
    // window still produces a real, finite number, not swept up by
    // the new guard being too strict.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "FCAST-002", "Forecast Item 2", 50, 100, 200);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    let result = crate::forecast::moving_average_forecast(&conn, &biz, &uid, "sales", "revenue", "month", 3)
        .unwrap();
    assert!(result.forecast_next.is_finite(), "forecast_next must be a real number, got {}", result.forecast_next);
    assert_eq!(result.forecast_next, 200.0);
}

#[test]
fn test_exponential_smoothing_forecast_rejects_out_of_range_alpha() {
    // THE BUG THIS FIXES: alpha reaches this function straight from a
    // query-string `.parse::<f64>()`, which happily accepts "nan",
    // "inf", 0, and negative values as "valid" floats — none of which
    // match this function's own documented contract of "alpha in
    // (0,1]". A NaN alpha poisons every period of the running
    // exponential average, and (same as the window=0 case above)
    // silently becomes a JSON `null` rather than a visible error.
    // Covers every boundary this function's guard actually checks.
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);
    let _ = seed_inventory_item(&conn, &biz, "FCAST-003", "Forecast Item 3", 50, 100, 200);

    for bad_alpha in [f64::NAN, f64::INFINITY, 0.0, -0.5, 1.5] {
        let err = crate::forecast::exponential_smoothing_forecast(
            &conn, &biz, &uid, "sales", "revenue", "month", bad_alpha,
        )
        .unwrap_err();
        assert!(err.to_string().contains("alpha"), "alpha={bad_alpha}: unexpected error: {err}");
    }
}

#[test]
fn test_exponential_smoothing_forecast_accepts_the_full_valid_range() {
    // The boundary itself (alpha=1.0) and a normal mid-range value
    // must both still work — the guard is "outside (0,1]", not an
    // overly strict "somewhere strictly inside (0,1)".
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let inv_id = seed_inventory_item(&conn, &biz, "FCAST-004", "Forecast Item 4", 50, 100, 200);
    let req = crate::pos::CheckoutRequest {
        items: vec![crate::pos::CartItem { inventory_record_id: inv_id, quantity: 1 }],
        payment_method: Some("Cash".into()),
        customer: None,
        customer_phone: None,
        allow_oversell: false,
        on_credit: false,
        due_date: None,
    };
    crate::pos::checkout(&mut conn, &biz, &uid, req).unwrap();

    for ok_alpha in [1.0, 0.5, 0.01] {
        let result = crate::forecast::exponential_smoothing_forecast(
            &conn, &biz, &uid, "sales", "revenue", "month", ok_alpha,
        )
        .unwrap();
        assert!(result.forecast_next.is_finite(), "alpha={ok_alpha}: forecast_next must be finite, got {}", result.forecast_next);
    }
}
